//! Main GUI, styled to match the flax extension (same palette, radii,
//! spacing and type scale as `panel.css`, light and dark):
//!
//! - a 44px toolbar-style header (colza logo, settings button at the right);
//! - a body split by a 1px vertical line (like flax's sidebar border):
//!   - left (white, full height): a WCAG 2.x / APCA segmented control
//!     with a "swap foreground & background" icon button beside it,
//!     contrast preview, ratio or Lc, and the method's levels as
//!     pass/fail tags;
//!   - right (grey): Foreground and Background "cards", each as tall as
//!     its content (format dropdown, swatch, text field, eyedropper,
//!     three sliders for the chosen format's model, R/G/B, H/S/L or
//!     OkLCh's L/C/H, plus Alpha, with gradient rails and value
//!     readouts).
//!
//! The settings button swaps the body for a settings page (theme,
//! text-size shortcuts, and the capture permissions with a "Reset" for
//! each, see `permissions.rs`; version and
//! repository link in its header), saved by `prefs.rs`. Back, or Escape,
//! returns. The page scrolls, since it outgrew the fixed window.
//!
//! Picking a color opens the magnifier as a second egui viewport so the
//! main window stays visible while the loupe is up.
//!
//! The theme follows the system (light/dark, read by `system_theme.rs`)
//! unless set otherwise in the settings.
//! The window has a fixed size in points (`WINDOW_SIZE`), scaled as a
//! whole by the zoom: the desktop's text size (`text_scale.rs`) times the
//! user's own Ctrl+/Ctrl- zoom. The window is resized to match whenever
//! the zoom changes, so nothing gets clipped.
//!
//! Fonts: Roboto Regular and Roboto Bold are embedded from `assets/`.
//!
//! Run with: `cargo run`

use std::sync::Arc;

use eframe::egui;
use image::RgbImage;

use crate::color::{format_number, Hsl, Oklch, Rgb};
use crate::magnifier::MagnifierState;
use crate::screencast::ScreenSource;

/// Colors shared with flax's `panel.css` (`:root` for light,
/// `prefers-color-scheme: dark` / `html.theme-dark` for dark).
#[derive(Clone, Copy)]
struct Palette {
    /// `--bg-color`: cards, text fields.
    bg: egui::Color32,
    /// `--toolbar-bg`: header and window body.
    toolbar_bg: egui::Color32,
    /// `--border-color`.
    border: egui::Color32,
    /// `--accent-color`.
    accent: egui::Color32,
    /// `--accent-bg`.
    accent_bg: egui::Color32,
    /// `--text-primary`.
    text: egui::Color32,
    /// `--text-secondary`.
    text_secondary: egui::Color32,
    /// `--focus-color`.
    focus: egui::Color32,
    /// `.best-practice-tag` colors (swapped in dark mode, as in flax).
    tag_fg: egui::Color32,
    tag_bg: egui::Color32,
    /// `--link-color`.
    link: egui::Color32,
}

impl Palette {
    const fn light() -> Self {
        Self {
            bg: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
            toolbar_bg: egui::Color32::from_rgb(0xF1, 0xF3, 0xF4),
            border: egui::Color32::from_rgb(0xE0, 0xE0, 0xE0),
            accent: egui::Color32::from_rgb(0x3E, 0x47, 0xFF),
            accent_bg: egui::Color32::from_rgb(0xEA, 0xEE, 0xFF),
            text: egui::Color32::from_rgb(0x20, 0x21, 0x24),
            text_secondary: egui::Color32::from_rgb(0x5F, 0x63, 0x68),
            focus: egui::Color32::from_rgb(0x00, 0x00, 0x00),
            tag_fg: egui::Color32::from_rgb(0x3E, 0x47, 0xFF),
            tag_bg: egui::Color32::from_rgb(0xEA, 0xEE, 0xFF),
            link: egui::Color32::from_rgb(0x19, 0x67, 0xD2),
        }
    }

    const fn dark() -> Self {
        Self {
            bg: egui::Color32::from_rgb(0x20, 0x21, 0x24),
            toolbar_bg: egui::Color32::from_rgb(0x29, 0x2A, 0x2D),
            border: egui::Color32::from_rgb(0x3C, 0x40, 0x43),
            accent: egui::Color32::from_rgb(0xEA, 0xEE, 0xFF),
            accent_bg: egui::Color32::from_rgb(0x3E, 0x47, 0xFF),
            text: egui::Color32::from_rgb(0xE8, 0xEA, 0xED),
            text_secondary: egui::Color32::from_rgb(0x9A, 0xA0, 0xA6),
            focus: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
            tag_fg: egui::Color32::from_rgb(0x3E, 0x47, 0xFF),
            tag_bg: egui::Color32::from_rgb(0xEA, 0xEE, 0xFF),
            link: egui::Color32::from_rgb(0x8A, 0xB4, 0xFB),
        }
    }

    const fn new(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }
}

/// Background of the "Fail" tag: flax's `--highlight` red in light mode,
/// kept for both themes so white text on it stays above 4.5:1.
const FAIL_BG: egui::Color32 = egui::Color32::from_rgb(0xD9, 0x30, 0x25);
/// Neutral hover fill for icon-only buttons (flax: `rgba(128,128,128,.15)`).
const ICON_HOVER: egui::Color32 = egui::Color32::from_rgba_premultiplied(19, 19, 19, 38);

/// Horizontal gap between the layout columns.
const COLUMN_GAP: f32 = 12.0;
/// Height of the header toolbar. flax uses 36px, but the logo needs more
/// air around it than the plain-text title had.
const HEADER_HEIGHT: f32 = 44.0;
/// Height of swatch, text field and icon buttons.
const CONTROL_SIZE: f32 = 32.0;
/// Inner margin of cards and of the summary section (flax: 16px).
const CARD_MARGIN: f32 = 16.0;
/// Width of the left "contrast" summary section, including its right
/// border (flax: the sidebar, separated from the detail pane by a 1px
/// line).
const SUMMARY_WIDTH: f32 = 260.0;
/// Fixed window size (the window is not resizable). The height fits the
/// tallest section: header (44) + a color card (318) + its 16px margins
/// above and below. The summary section is shorter (~290 of content)
/// since the swap button moved into its header row.
/// The width is set by the longest text the color field must show whole:
/// an OkLCh value like `oklch(56.36% 0.29123 302.15)` measures ~163px in
/// Roboto 12px, and at 840px wide the field only had 140px of text area.
const WINDOW_SIZE: [f32; 2] = [896.0, 394.0];
/// Extra vertical space between two sliders (on top of the card's spacing).
const SLIDER_GAP: f32 = 6.0;
/// Space between the vertical line and the color cards.
const COLORS_PAD_LEFT: f32 = 28.0;
/// Size of the icon buttons in the header (settings, back).
const HEADER_BUTTON_SIZE: f32 = 28.0;
/// Height of the logo in the header. Its wordmark fills the middle ~37%
/// of the image's height, so at 22pt the letters stand about as tall as
/// the 12px bold title it replaces.
const LOGO_HEIGHT: f32 = 22.0;
/// `repository` from Cargo.toml, empty when unset (then the settings
/// page shows no link rather than a guessed one).
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Which page fills the window below the header.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Main,
    Settings,
}

/// Stable ids for the two page-switching buttons, so focus can be moved
/// onto the one that appears when the page changes (see `App::navigate`).
fn settings_button_id() -> egui::Id {
    egui::Id::new("colza-settings-button")
}
fn back_button_id() -> egui::Id {
    egui::Id::new("colza-back-button")
}

/// Stable id for the magnifier's viewport, so the same native window can be
/// referred to across frames (opened once, updated, then closed).
fn magnifier_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("magnifier")
}

/// Font family backed by Roboto Bold. egui has no synthetic bold, so a
/// real bold weight needs its own family (see [`setup_fonts`]).
fn bold() -> egui::FontFamily {
    egui::FontFamily::Name("roboto-bold".into())
}

/// Which color card a pick is targeting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Fg,
    Bg,
}

/// What the app is doing right now, independent of which window(s) are on
/// screen. The main window is always shown; this only controls whether the
/// magnifier viewport is also shown and where its frames come from.
enum Mode {
    /// Just the main window.
    Normal,
    /// "Pick" was clicked for `Target`; a background task is opening a
    /// capture session. The main window stays responsive during this since
    /// capture happens off the UI thread.
    Capturing(
        Target,
        std::sync::mpsc::Receiver<anyhow::Result<(ScreenSource, RgbImage)>>,
    ),
    /// First frame in hand; the magnifier viewport is open and sampling
    /// `state` for `Target`.
    ///
    /// Holding the `ScreenSource` here for the whole time the loupe is up
    /// is what makes it a live view: `ui()` pulls the newest frame from
    /// it each repaint. Dropping this variant (pick or cancel) stops the
    /// stream and closes the portal session.
    Magnifying(Target, Box<MagnifierState>, ScreenSource),
}

/// How a color is written in the text field, and which model the three
/// sliders under it edit.
///
/// HSL rather than HSV: both are the same cylinder-over-sRGB idea, but
/// HSL is the one CSS speaks (`hsl()`), so its text pastes straight into
/// a stylesheet, like every other format here. OkLCh is the perceptual
/// one: its L tracks how light a color *looks* across hues, which makes
/// it the better slider to reach for when nudging a pair toward a passing
/// ratio without changing its character.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorFormat {
    Hex,
    Rgb,
    Hsl,
    Oklch,
}

/// One slider's range and units. Every range starts at 0.
struct Channel {
    name: &'static str,
    max: f64,
    /// Arrow-key step, and the grid pointer drags snap to.
    step: f64,
    /// Shift+arrow step.
    big_step: f64,
    /// Decimals in the readout. The text field always carries the full
    /// precision, so the readout can stay compact.
    decimals: usize,
    unit: &'static str,
}

impl Channel {
    const fn new(name: &'static str, max: f64, step: f64, big_step: f64, decimals: usize, unit: &'static str) -> Self {
        Self { name, max, step, big_step, decimals, unit }
    }

    fn readout(&self, value: f64) -> String {
        format!("{}{}", format_number(value, self.decimals), self.unit)
    }
}

const RGB_CHANNELS: [Channel; 3] = [
    Channel::new("Red", 255.0, 1.0, 10.0, 0, ""),
    Channel::new("Green", 255.0, 1.0, 10.0, 0, ""),
    Channel::new("Blue", 255.0, 1.0, 10.0, 0, ""),
];
const HSL_CHANNELS: [Channel; 3] = [
    Channel::new("Hue", 360.0, 1.0, 10.0, 0, "°"),
    Channel::new("Saturation", 100.0, 1.0, 10.0, 0, "%"),
    Channel::new("Lightness", 100.0, 1.0, 10.0, 0, "%"),
];
/// Chroma tops out at 0.4 because that's what CSS calls `100%`; sRGB's
/// own maximum is ~0.32 (blue), so the top of the rail is always out of
/// gamut, which the rail shows by flattening out.
const OKLCH_CHANNELS: [Channel; 3] = [
    Channel::new("Lightness", 100.0, 1.0, 10.0, 1, "%"),
    Channel::new("Chroma", 0.4, 0.005, 0.05, 3, ""),
    Channel::new("Hue", 360.0, 1.0, 10.0, 0, "°"),
];
const ALPHA_CHANNEL: Channel = Channel::new("Alpha", 255.0, 1.0, 10.0, 0, "");

impl ColorFormat {
    const ALL: [Self; 4] = [Self::Hex, Self::Rgb, Self::Hsl, Self::Oklch];

    fn name(self) -> &'static str {
        match self {
            Self::Hex => "HEX",
            Self::Rgb => "RGB",
            Self::Hsl => "HSL",
            Self::Oklch => "OKLCH",
        }
    }

    fn channels(self) -> &'static [Channel; 3] {
        match self {
            Self::Hex | Self::Rgb => &RGB_CHANNELS,
            Self::Hsl => &HSL_CHANNELS,
            Self::Oklch => &OKLCH_CHANNELS,
        }
    }

    /// `c` in this format's slider units (see the `*_CHANNELS` tables).
    fn components(self, c: Rgb) -> [f64; 3] {
        match self {
            Self::Hex | Self::Rgb => [c.r, c.g, c.b].map(f64::from),
            Self::Hsl => {
                let Hsl { h, s, l } = c.to_hsl();
                [h, s * 100.0, l * 100.0]
            }
            Self::Oklch => {
                let Oklch { l, c, h } = c.to_oklch();
                [l * 100.0, c, h]
            }
        }
    }

    /// The color that components `v` name, and whether it lay outside
    /// sRGB and had to be mapped in. Only OkLCh can be out of gamut: RGB
    /// and HSL both describe exactly the sRGB cube.
    fn to_rgb(self, v: [f64; 3]) -> (Rgb, bool) {
        match self {
            Self::Hex | Self::Rgb => {
                let [r, g, b] = v.map(|c| c.round().clamp(0.0, 255.0) as u8);
                (Rgb::new(r, g, b), false)
            }
            Self::Hsl => (Rgb::from_hsl(self.hsl(v)), false),
            Self::Oklch => Rgb::from_oklch_mapped(self.oklch(v)),
        }
    }

    fn hsl(self, [h, s, l]: [f64; 3]) -> Hsl {
        Hsl { h, s: s / 100.0, l: l / 100.0 }
    }

    fn oklch(self, [l, c, h]: [f64; 3]) -> Oklch {
        Oklch { l: l / 100.0, c, h }
    }

    /// Text for components `v`.
    ///
    /// Written from the components rather than from the 8-bit color, so
    /// the field always agrees with the sliders: an OkLCh color dragged
    /// out of gamut reads as what was asked for (as CSS would hold it),
    /// and a gray's hue survives in HSL instead of collapsing to 0.
    fn format(self, v: [f64; 3]) -> String {
        match self {
            Self::Hex => self.to_rgb(v).0.to_hex(),
            Self::Rgb => {
                let (c, _) = self.to_rgb(v);
                format!("rgb({}, {}, {})", c.r, c.g, c.b)
            }
            Self::Hsl => self.hsl(v).to_css(),
            Self::Oklch => self.oklch(v).to_css(),
        }
    }

    /// Components from text written in *this* format, or `None`.
    ///
    /// Kept apart from [`parse_color`] (any format, to an `Rgb`) because
    /// going through 8-bit RGB would throw away exactly what the user
    /// typed: a hue on a gray, an out-of-gamut chroma.
    fn parse(self, text: &str) -> Option<[f64; 3]> {
        match self {
            Self::Hex => {
                let t = text.trim();
                Some(Self::Rgb.components(Rgb::from_hex(t.strip_prefix('#').unwrap_or(t)).ok()?))
            }
            Self::Rgb => Some(Self::Rgb.components(parse_rgb_function(text)?)),
            Self::Hsl => {
                let Hsl { h, s, l } = Hsl::parse_css(text)?;
                Some([h, s * 100.0, l * 100.0])
            }
            Self::Oklch => {
                let Oklch { l, c, h } = Oklch::parse_css(text)?;
                Some([l * 100.0, c, h])
            }
        }
    }
}

/// Parses any supported format: `#rrggbb`, `rrggbb`, `rgb(r, g, b)`,
/// `hsl(...)` or `oklch(...)` (see `color.rs` for the accepted spellings).
fn parse_color(text: &str) -> Option<Rgb> {
    ColorFormat::ALL
        .into_iter()
        .find_map(|f| f.parse(text).map(|v| f.to_rgb(v).0))
}

/// Parses `rgb(r, g, b)`.
fn parse_rgb_function(text: &str) -> Option<Rgb> {
    let t = text.trim();
    if !t.to_lowercase().starts_with("rgb") {
        return None;
    }
    let inner = t.split_once('(')?.1.strip_suffix(')')?;
    let mut parts = inner.split(',').map(|p| p.trim().parse::<u16>());
    let r = parts.next()?.ok()?;
    let g = parts.next()?.ok()?;
    let b = parts.next()?.ok()?;
    if parts.next().is_some() {
        return None; // extra component: not a valid rgb(...)
    }
    if r > 255 || g > 255 || b > 255 {
        return None;
    }
    Some(Rgb::new(r as u8, g as u8, b as u8))
}

/// `top` at opacity `alpha` composited over an opaque `under`.
fn blend(top: Rgb, alpha: u8, under: Rgb) -> Rgb {
    let a = alpha as f32 / 255.0;
    let mix = |t: u8, u: u8| (t as f32 * a + u as f32 * (1.0 - a)).round() as u8;
    Rgb::new(mix(top.r, under.r), mix(top.g, under.g), mix(top.b, under.b))
}

/// APCA Lc as shown: the magnitude, truncated (never rounded up) to one
/// decimal, for the same reason as [`format_ratio`]: an Lc of 74.96
/// fails "75+", so it must not read "75".
///
/// APCA's raw value is signed (negative = light text on dark), but the
/// sign only encodes polarity, which the "Aa" preview already shows, and
/// the levels apply to the magnitude. Shown next to "Lc 75+" thresholds,
/// "Lc -80" reads as failing when it passes, and swapping the colors
/// would flip it for no visible reason.
fn format_lc(lc: f64) -> String {
    let truncated = (lc.abs() * 10.0 + 1e-6).floor() / 10.0;
    format!("Lc {}", format_number(truncated, 1))
}

/// Ratio as shown to the user: truncated (never rounded up) to two
/// decimals, trailing zeros dropped. WCAG says 4.499 must not read "4.5".
fn format_ratio(ratio: f64) -> String {
    let truncated = (ratio * 100.0 + 1e-6).floor() / 100.0;
    let s = format!("{truncated:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn color32(c: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}

/// Round-handle slider painted by hand: a gradient rail through `track`
/// (evenly spaced stops, so the rail previews what the channel does), a
/// handle with an accent outline, a value readout, and pointer + keyboard
/// support (arrows ±`step`, Shift+arrows ±`big_step`) with an accessible
/// name/value. `value` runs from 0 to `channel.max`. Returns true if
/// `value` changed.
///
/// `readout` is passed in rather than derived from `channel` so the
/// caller can annotate it (OkLCh's "out of sRGB"). Like the rail, it
/// reflects the value at the start of the frame; a change repaints.
fn channel_slider(
    ui: &mut egui::Ui,
    p: &Palette,
    channel: &Channel,
    a11y_prefix: &str,
    value: &mut f64,
    readout: &str,
    track: &[egui::Color32],
) -> bool {
    const HANDLE_RADIUS: f32 = 9.0;

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;

        // Name on the left, current value on the right.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(channel.name).size(11.0).color(p.text_secondary));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(readout).size(11.0).color(p.text));
            });
        });

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 2.0 * HANDLE_RADIUS),
            egui::Sense::click_and_drag(),
        );
        let label = format!("{a11y_prefix} {}", channel.name);
        resp.widget_info(|| egui::WidgetInfo::slider(true, *value, label.clone()));
        // `WidgetInfo::slider` only carries the raw number. The range lets
        // a screen reader place it ("50%" of the way), the step matches
        // the arrow keys, and the value text gives the units and notes the
        // sighted readout shows ("210°", "0.258 · out of sRGB") instead
        // of a bare "210" or "0.258".
        a11y(ui, resp.id, |node| {
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(channel.max);
            node.set_numeric_value_step(channel.step);
            node.set_value(readout);
        });

        let x0 = rect.left() + HANDLE_RADIUS;
        let x1 = rect.right() - HANDLE_RADIUS;
        let mut changed = false;
        let mut set = |v: f64, value: &mut f64| {
            let v = v.clamp(0.0, channel.max);
            if v != *value {
                *value = v;
                changed = true;
            }
        };

        if resp.is_pointer_button_down_on() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0) as f64;
                // Snapped to `step` so a drag lands on the same values the
                // arrow keys produce, not on whatever a pixel maps to.
                set((t * channel.max / channel.step).round() * channel.step, value);
            }
        }

        if resp.has_focus() {
            // Keep egui from using the arrows to move focus to another widget.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
            let delta: f64 = ui.input_mut(|input| {
                let mut d = 0.0;
                for (key, sign) in [
                    (egui::Key::ArrowLeft, -1.0),
                    (egui::Key::ArrowDown, -1.0),
                    (egui::Key::ArrowRight, 1.0),
                    (egui::Key::ArrowUp, 1.0),
                ] {
                    if input.consume_key(egui::Modifiers::SHIFT, key) {
                        d += channel.big_step * sign;
                    } else if input.consume_key(egui::Modifiers::NONE, key) {
                        d += channel.step * sign;
                    }
                }
                d
            });
            if delta != 0.0 {
                // Snap first: a value derived from a picked color (say a
                // hue of 213.7°) steps to 214/213, not 214.7/212.7.
                let snapped = (*value / channel.step).round() * channel.step;
                set(snapped + delta, value);
            }
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        let t = (*value / channel.max).clamp(0.0, 1.0) as f32;
        let cx = x0 + t * (x1 - x0);

        // Gradient rail, one quad per pair of stops. It stops at the
        // handle's travel limits, so its square ends are always hidden
        // under the handle.
        let rail = egui::Rect::from_min_max(egui::pos2(x0, cy - 3.0), egui::pos2(x1, cy + 3.0));
        let mut mesh = egui::Mesh::default();
        let last = track.len().saturating_sub(1).max(1) as f32;
        for (i, &color) in track.iter().enumerate() {
            let x = rail.left() + rail.width() * i as f32 / last;
            mesh.colored_vertex(egui::pos2(x, rail.top()), color);
            mesh.colored_vertex(egui::pos2(x, rail.bottom()), color);
            if i > 0 {
                let k = 2 * i as u32;
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k + 1, k);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.rect_stroke(
            rail,
            0.0,
            egui::Stroke::new(1.0_f32, p.text_secondary),
            egui::StrokeKind::Outside,
        );

        let center = egui::pos2(cx, cy);
        painter.circle_filled(center, HANDLE_RADIUS - 2.0, p.bg);
        painter.circle_stroke(center, HANDLE_RADIUS - 2.0, egui::Stroke::new(2.0, p.accent));
        if resp.has_focus() {
            painter.circle_stroke(center, HANDLE_RADIUS + 1.0, egui::Stroke::new(2.0, p.focus));
        }

        changed
    })
    .inner
}

/// SVG path from `eye-dropper-solid-full.svg` (Font Awesome Free 7.3.1).
/// The SVG's M/L/C commands are sampled into egui paths at runtime, so the
/// icon uses the supplied vector shape rather than a hand-drawn approximation.
const EYEDROPPER_SVG_PATH: &str = "M405.6 93.2 L304 194.8 L294.6 185.4 C282.1 172.9 261.8 172.9 249.3 185.4 C236.8 197.9 236.8 218.2 249.3 230.7 L409.3 390.7 C421.8 403.2 442.1 403.2 454.6 390.7 C467.1 378.2 467.1 357.9 454.6 345.4 L445.2 336 L546.8 234.4 C585.8 195.4 585.8 132.2 546.8 93.3 C507.8 54.4 444.6 54.3 405.7 93.3 z M119.4 387.3 C104.4 402.3 96 422.7 96 443.9 L96 486.3 L69.4 526.2 C60.9 538.9 62.6 555.8 73.4 566.6 C84.2 577.4 101.1 579.1 113.8 570.6 L153.7 544 L196.1 544 C217.3 544 237.7 535.6 252.7 520.6 L362.1 411.2 L316.8 365.9 L207.4 475.3 C204.4 478.3 200.3 480 196.1 480 L160 480 L160 443.9 C160 439.7 161.7 435.6 164.7 432.6 L274.1 323.2 L228.8 277.9 z";

/// Parses the small SVG path subset used by the supplied icon (M, L, C, z).
fn eyedropper_svg_polygons() -> Vec<Vec<egui::Pos2>> {
    fn number(bytes: &[u8], i: &mut usize) -> f32 {
        while *i < bytes.len() && (bytes[*i].is_ascii_whitespace() || bytes[*i] == b',') {
            *i += 1;
        }
        let start = *i;
        while *i < bytes.len()
            && (bytes[*i].is_ascii_digit() || matches!(bytes[*i], b'.' | b'-' | b'+' | b'e' | b'E'))
        {
            *i += 1;
        }
        std::str::from_utf8(&bytes[start..*i]).unwrap().parse().unwrap()
    }

    let bytes = EYEDROPPER_SVG_PATH.as_bytes();
    let mut i = 0;
    let mut command = b' ';
    let mut current = egui::pos2(0.0, 0.0);
    let mut start = current;
    let mut points = Vec::new();
    let mut polygons = Vec::new();

    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i].is_ascii_alphabetic() {
            command = bytes[i];
            i += 1;
        }
        match command {
            b'M' | b'L' => {
                let point = egui::pos2(number(bytes, &mut i), number(bytes, &mut i));
                if command == b'M' {
                    if !points.is_empty() {
                        polygons.push(std::mem::take(&mut points));
                    }
                    start = point;
                    // Coordinates after an M's first pair are implicit L's.
                    command = b'L';
                }
                current = point;
                points.push(point);
            }
            b'C' => {
                let p0 = current;
                let p1 = egui::pos2(number(bytes, &mut i), number(bytes, &mut i));
                let p2 = egui::pos2(number(bytes, &mut i), number(bytes, &mut i));
                let p3 = egui::pos2(number(bytes, &mut i), number(bytes, &mut i));
                // Cubic Bézier, sampled at 8 points.
                for step in 1..=8 {
                    let t = step as f32 / 8.0;
                    let u = 1.0 - t;
                    points.push(egui::pos2(
                        u * u * u * p0.x + 3.0 * u * u * t * p1.x + 3.0 * u * t * t * p2.x + t * t * t * p3.x,
                        u * u * u * p0.y + 3.0 * u * u * t * p1.y + 3.0 * u * t * t * p2.y + t * t * t * p3.y,
                    ));
                }
                current = p3;
            }
            b'z' | b'Z' => {
                current = start;
                if !points.is_empty() {
                    polygons.push(std::mem::take(&mut points));
                }
                command = b' ';
            }
            _ => break,
        }
    }
    if !points.is_empty() {
        polygons.push(points);
    }
    polygons
}

fn paint_eyedropper(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    // Renders the SVG path 18px wide, centered inside the hit target.
    // The visible path spans about 526 units of the 640×640 viewBox.
    let scale = 18.0 / 526.0;
    let origin = rect.center() - egui::vec2(323.0 * scale, 315.0 * scale);
    for polygon in eyedropper_svg_polygons() {
        let points = polygon
            .into_iter()
            .map(|point| origin + egui::vec2(point.x * scale, point.y * scale))
            .collect();
        painter.add(egui::Shape::Path(egui::epaint::PathShape {
            points,
            closed: true,
            fill: color,
            stroke: egui::Stroke::NONE.into(),
        }));
    }
}

/// A double arrow (⇄) drawn with strokes: no icon font needed.
fn paint_swap(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    let at = |x: f32, y: f32| center + egui::vec2(x, y);
    let stroke = egui::Stroke::new(2.0, color);
    // Top arrow, pointing right.
    painter.line_segment([at(-9.0, -4.0), at(9.0, -4.0)], stroke);
    painter.line_segment([at(5.0, -8.0), at(9.0, -4.0)], stroke);
    painter.line_segment([at(5.0, 0.0), at(9.0, -4.0)], stroke);
    // Bottom arrow, pointing left.
    painter.line_segment([at(9.0, 4.0), at(-9.0, 4.0)], stroke);
    painter.line_segment([at(-5.0, 0.0), at(-9.0, 4.0)], stroke);
    painter.line_segment([at(-5.0, 8.0), at(-9.0, 4.0)], stroke);
}

/// Gear icon (settings), drawn with strokes: a thick ring for the body
/// and eight short, thick spokes past it for the teeth.
fn paint_gear(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    painter.circle_stroke(center, 5.0, egui::Stroke::new(3.0, color));
    let teeth = egui::Stroke::new(3.2, color);
    for i in 0..8 {
        let angle = i as f32 * std::f32::consts::FRAC_PI_4 + std::f32::consts::FRAC_PI_8;
        let dir = egui::vec2(angle.cos(), angle.sin());
        painter.line_segment([center + dir * 6.0, center + dir * 9.0], teeth);
    }
}

/// Left arrow (back), like flax's settings header.
fn paint_back_arrow(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    let at = |x: f32, y: f32| center + egui::vec2(x, y);
    let stroke = egui::Stroke::new(2.0, color);
    painter.line_segment([at(-7.0, 0.0), at(7.0, 0.0)], stroke);
    painter.line_segment([at(-7.0, 0.0), at(-2.0, -5.0)], stroke);
    painter.line_segment([at(-7.0, 0.0), at(-2.0, 5.0)], stroke);
}

/// Frameless, icon-only square button (swap, settings, back): neutral
/// fill on hover, focus ring when focused. With no visible text, its name
/// reaches assistive tech through `widget_info` and sighted users through
/// the tooltip. `id` pins the widget id when focus must be moved onto it
/// from elsewhere.
fn icon_button(
    ui: &mut egui::Ui,
    p: &Palette,
    id: Option<egui::Id>,
    size: f32,
    label: &str,
    paint: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    let (rect, resp) = match id {
        Some(id) => {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
            (rect, ui.interact(rect, id, egui::Sense::click()))
        }
        None => ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click()),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));

    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, ICON_HOVER);
    }
    let color = if resp.hovered() { p.text } else { p.text_secondary };
    paint(ui.painter(), rect, color);
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(2.0, p.focus),
            egui::StrokeKind::Outside,
        );
    }
    resp.on_hover_text(label)
}

/// A `ComboBox` over `all`, with the keyboard behavior of a native
/// `<select>`, which egui's ComboBox only partly has. Returns the
/// dropdown button's response and whether `value` changed.
///
/// - Closed and focused: Up/Down step through `all` in place (clamped, not
///   wrapped); egui has no keyboard handling here besides opening.
/// - Opened from the keyboard (Space/Enter): focus moves onto the current
///   option, so it shows where the user is and arrows go on from there.
///   egui would leave it on the button, where nothing marks it while the
///   list covers the page: it reads as lost.
/// - Open: arrows move between the options only (see [`list_arrow_keys`]).
/// - Enter/Space on an option picks it and closes the list. egui's
///   `CloseOnClick` only counts pointer clicks, so it would stay open.
/// - Once the list closes (pick, Escape, click), focus goes back to the
///   button, as a native control does. Otherwise it's really lost: Escape
///   clears egui's focus outright, and a picked option is a widget that
///   no longer exists. Not done if focus has meanwhile gone elsewhere
///   (Tab out of the list, a click on another widget).
fn select_box<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    combo: egui::ComboBox,
    value: &mut T,
    all: &[T],
    option_text: impl Fn(T) -> egui::RichText,
) -> (egui::Response, bool) {
    // The button's id, needed inside the list before the button has been
    // drawn this frame: remembered from the last one, under a key tied to
    // this spot in the layout.
    let button_key = ui.next_auto_id().with("colza-select-button");
    let button_id: Option<egui::Id> = ui.data(|d| d.get_temp(button_key));

    let mut option_ids = Vec::with_capacity(all.len());
    let mut changed = false;
    let inner = combo.show_ui(ui, |ui| {
        for &v in all {
            let option = ui.selectable_value(value, v, option_text(v));
            changed |= option.changed();
            if option.clicked() && !option.clicked_by(egui::PointerButton::Primary) {
                ui.close();
            }
            option_ids.push(option.id);
        }
        let selected = all.iter().position(|v| v == value);
        list_arrow_keys(ui, &option_ids, button_id, selected);
    });
    let open = inner.inner.is_some();
    let resp = inner.response;
    ui.data_mut(|d| d.insert_temp(button_key, resp.id));

    if !open && resp.has_focus() {
        let step: isize = ui.input_mut(|input| {
            let mut d = 0;
            if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                d -= 1;
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                d += 1;
            }
            d
        });
        if step != 0 {
            let i = all.iter().position(|v| v == value).unwrap_or(0) as isize;
            let next = all[(i + step).clamp(0, all.len() as isize - 1) as usize];
            changed |= next != *value;
            *value = next;
        }
    }

    if open && resp.clicked() && !resp.clicked_by(egui::PointerButton::Primary) {
        if let Some(i) = all.iter().position(|v| v == value) {
            ui.memory_mut(|m| m.request_focus(option_ids[i]));
            ui.ctx().request_repaint();
        }
    }

    let focus_key = resp.id.with("colza-select-focus");
    let focused = ui.memory(|m| m.focused());
    if open {
        if let Some(f) = focused.filter(|f| *f == resp.id || option_ids.contains(f)) {
            ui.data_mut(|d| d.insert_temp(focus_key, Some(f)));
        }
    } else if let Some(last) = ui.data_mut(|d| d.remove_temp::<Option<egui::Id>>(focus_key)).flatten() {
        if focused.is_none() || focused == Some(last) {
            ui.memory_mut(|m| m.request_focus(resp.id));
        }
    }

    (resp, changed)
}

/// Arrow keys inside an open dropdown list whose options are `option_ids`
/// (in order), `button` being its dropdown button and `selected` the
/// index of the current value. Up/Down move focus between the options,
/// clamped at the ends; Home/End jump to them; from the button, either
/// arrow lands on the current option. Left/Right do nothing.
///
/// Must run inside the list, after its options: egui's own handling of
/// arrows (its spatial focus move at the end of the frame, which happily
/// leaves the list for the widgets under it, and since 0.37 its menu
/// navigation, which runs just after the list's contents) is cancelled
/// from here with `move_focus(None)` before either acts. Normally
/// `keep_arrows_off_focus` already keeps egui from seeing the arrows, but
/// not on the first frame after focus moves to an option: egui only
/// accepts a focus-lock filter for a widget focused since the last frame.
fn list_arrow_keys(
    ui: &egui::Ui,
    option_ids: &[egui::Id],
    button: Option<egui::Id>,
    selected: Option<usize>,
) {
    let Some(last) = option_ids.len().checked_sub(1) else {
        return;
    };
    let focused = ui.memory(|m| m.focused());
    let current = focused.and_then(|f| option_ids.iter().position(|id| *id == f));
    if current.is_none() && (focused.is_none() || focused != button) {
        return;
    }
    let (step, home, end, sideways) = ui.input_mut(|input| {
        let mut d = 0isize;
        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            d -= 1;
        }
        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            d += 1;
        }
        (
            d,
            input.consume_key(egui::Modifiers::NONE, egui::Key::Home),
            input.consume_key(egui::Modifiers::NONE, egui::Key::End),
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                | input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
        )
    });
    let target = if home {
        Some(0)
    } else if end {
        Some(last)
    } else if step != 0 {
        Some(match current {
            Some(i) => (i as isize + step).clamp(0, last as isize) as usize,
            None => selected.unwrap_or(0).min(last),
        })
    } else {
        None
    };
    if target.is_none() && !sideways {
        return;
    }
    ui.memory_mut(|m| {
        m.move_focus(egui::FocusDirection::None);
        if let Some(t) = target {
            m.request_focus(option_ids[t]);
        }
    });
    // An idle frame right away, so the new option has been focused for a
    // frame (and has its filter) before the next key, held or not, arrives.
    ui.ctx().request_repaint();
}

/// A key drawn as a keycap, for the keyboard-shortcut list.
fn keycap(ui: &mut egui::Ui, p: &Palette, key: &str) {
    egui::Frame::new()
        .fill(p.bg)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(key).size(11.0).family(bold()).color(p.text));
        });
}

/// GitHub's official marks, as downloaded from https://github.com/logos
/// (`github-mark.png` and `github-mark-white.png` in its archive). Kept
/// unmodified, as GitHub's usage terms require: no recoloring, so the
/// hover state below changes the background only, never the mark.
const GITHUB_MARK: &[u8] = include_bytes!("../assets/github-mark.png");
const GITHUB_MARK_WHITE: &[u8] = include_bytes!("../assets/github-mark-white.png");

/// The colza logo (pictogram and wordmark), one PNG per theme, each
/// rendered from its SVG with `rsvg-convert -h 160 <svg> -o <png>`.
/// `assets/colza.svg` is the original; neither variant uses it as is:
///
/// - `colza-light.svg` → `logo-colza.png`: the four pictogram colors
///   darkened in OkLCh lightness, hue kept, since the original yellow
///   (#EFFF2A) is 1.01:1 against the light header (#F1F3F4), i.e.
///   invisible. Each was lowered to a set contrast, keeping the original
///   yellow → green progression: #EFFF2A → #CBD908 (1.4:1), #D3F340 →
///   #B7D505 (1.5:1), #B6E656 → #A0CE3A (1.65:1), #9ADA6C → #89C85A
///   (1.8:1).
/// - `colza-dark.svg` → `logo-colza-dark.png`: the same four colors as
///   `colza-light.svg`, so the logo looks the same in both themes (7.2:1
///   and up on the dark header), with the wordmark's black (`.cls-3`)
///   changed to the dark palette's text color (#E8EAED) so it doesn't
///   vanish.
///
/// PNGs rather than the SVG itself, to render it without adding an SVG
/// rasterizer to the dependencies. 160px tall covers the 22pt it's drawn
/// at with zoom up to 3× on a 2× screen (132px) and some margin; the
/// mipmaps keep it smooth when it's drawn smaller.
const LOGO: &[u8] = include_bytes!("../assets/logo-colza.png");
const LOGO_DARK: &[u8] = include_bytes!("../assets/logo-colza-dark.png");

/// An embedded PNG, decoded and uploaded once, then cached in egui's
/// memory under `name`. `None` (logged) only if the asset is replaced by a
/// broken file, so callers can fall back to something drawn.
///
/// Mipmapped because the sources are far larger than they're drawn: plain
/// linear filtering samples too few source pixels when shrinking that
/// much, and the edges come out jagged.
fn embedded_texture(
    ctx: &egui::Context,
    name: &'static str,
    bytes: &'static [u8],
) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("embedded-texture", name));
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let decoded = match image::load_from_memory(bytes) {
        Ok(decoded) => decoded.to_rgba8(),
        Err(err) => {
            eprintln!("colza: could not decode {name}: {err}");
            return None;
        }
    };
    let size = [decoded.width() as usize, decoded.height() as usize];
    let texture = ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw()),
        egui::TextureOptions {
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..egui::TextureOptions::LINEAR
        },
    );
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

/// The GitHub mark for the current theme (black on light, white on dark).
fn github_mark(ctx: &egui::Context, dark: bool) -> Option<egui::TextureHandle> {
    if dark {
        embedded_texture(ctx, "github-mark-white", GITHUB_MARK_WHITE)
    } else {
        embedded_texture(ctx, "github-mark", GITHUB_MARK)
    }
}

/// The window's title: the colza logo for the current theme, exposed to
/// assistive tech as the level-1 heading "colza" (its text alternative,
/// and the same heading the plain-text title used to give). Falls back to
/// that text title if the image can't be decoded.
fn logo(ui: &mut egui::Ui, p: &Palette) {
    const NAME: &str = "colza";
    let texture = if ui.visuals().dark_mode {
        embedded_texture(ui.ctx(), "logo-colza-dark", LOGO_DARK)
    } else {
        embedded_texture(ui.ctx(), "logo-colza", LOGO)
    };
    let Some(texture) = texture else {
        let title = ui.label(egui::RichText::new(NAME).size(12.0).family(bold()).color(p.text));
        mark_heading(ui, title.id, 1, NAME);
        return;
    };
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(LOGO_HEIGHT * texture.aspect_ratio(), LOGO_HEIGHT),
        egui::Sense::hover(),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, NAME));
    // After `widget_info`, which would otherwise reset the role to image.
    mark_heading(ui, resp.id, 1, NAME);
    ui.painter().image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// GitHub mark linking to `url`, like flax's header. A link for
/// assistive tech (it opens a page, it doesn't act on the app), named
/// since the mark has no text.
fn github_link(ui: &mut egui::Ui, p: &Palette, url: &str) {
    const NAME: &str = "colza on GitHub";
    let Some(texture) = github_mark(ui.ctx(), ui.visuals().dark_mode) else {
        return;
    };
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(HEADER_BUTTON_SIZE, HEADER_BUTTON_SIZE),
        egui::Sense::click(),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, NAME));

    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, ICON_HOVER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let icon = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(20.0 * texture.aspect_ratio(), 20.0),
    );
    ui.painter().image(
        texture.id(),
        icon,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE, // no tint: the mark is shown as GitHub ships it
    );
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(2.0, p.focus),
            egui::StrokeKind::Outside,
        );
    }
    if resp.on_hover_text(format!("{NAME} ({url})")).clicked() {
        open_in_browser(url);
    }
}

/// Opens `url` in the user's browser through `xdg-open`, which every
/// Linux desktop provides (and which Flatpak routes through the OpenURI
/// portal). Done here rather than with egui's `open_url` because that
/// depends on eframe's optional `links` feature, and would silently do
/// nothing without it. The child is waited on from a thread so it doesn't
/// linger as a zombie.
fn open_in_browser(url: &str) {
    let url = url.to_owned();
    std::thread::spawn(move || {
        match std::process::Command::new("xdg-open").arg(&url).spawn() {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(err) => eprintln!("colza: could not open {url}: {err}"),
        }
    });
}

/// Small uppercase tag, like flax's `.best-practice-tag`. Exposes its
/// text to screen readers.
fn tag(ui: &mut egui::Ui, text: &str, fg: egui::Color32, bg: egui::Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_uppercase(),
        egui::FontId::new(9.0, bold()),
        fg,
    );
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    ui.painter().rect_filled(rect, 3.0, bg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Edits the AccessKit node egui built for `id` this frame. Must run
/// after the widget's own `widget_info` (which would overwrite it), and
/// is a no-op when no assistive technology is listening (egui only
/// builds the tree once one asks for it).
fn a11y(ui: &egui::Ui, id: egui::Id, edit: impl FnOnce(&mut egui::accesskit::Node)) {
    ui.ctx().accesskit_node_builder(id, edit);
}

/// Exposes a visible text label as a heading, so screen-reader users can
/// jump between the window's sections (H / Shift+H in Orca and NVDA). egui
/// never emits the heading role itself: `ui.heading()` only changes the
/// font.
///
/// The name is set explicitly: egui puts a label's text in its *value*,
/// which is where a label's name comes from, but not a heading's. Left
/// alone, it would be announced as "heading level 1" with no text.
fn mark_heading(ui: &egui::Ui, id: egui::Id, level: usize, text: &str) {
    a11y(ui, id, |node| {
        node.set_role(egui::accesskit::Role::Heading);
        node.set_level(level);
        node.set_label(text);
    });
}

/// Segmented control: a grey track holding one segment per option, the
/// selected one raised (card background, dark outline, bold text).
/// Returns true if the selection changed.
///
/// Built as a radio group for assistive tech, since that's what it is: a
/// `RadioGroup` named `group`, containing one radio button per segment
/// with its checked state. Keyboard follows the ARIA radio-group pattern
/// as far as egui allows: arrows move the selection *and* focus to the
/// neighboring segment (wrapping), Space/Enter select the focused one.
/// (ARIA would also take unselected segments out of the Tab order; egui
/// has no way to make a clickable widget unfocusable, so Tab visits both.)
fn segmented_control<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    p: &Palette,
    group: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    const HEIGHT: f32 = 26.0;
    const PAD_X: f32 = 10.0;
    const INSET: f32 = 2.0;
    const RADIUS: f32 = 6.0;

    // Every segment is sized for its *bold* label, so the control doesn't
    // change width as the bold moves with the selection.
    let font = |selected: bool| {
        if selected {
            egui::FontId::new(12.0, bold())
        } else {
            egui::FontId::proportional(12.0)
        }
    };
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| {
            let galley = ui.painter().layout_no_wrap((*label).to_owned(), font(true), p.text);
            galley.size().x + 2.0 * PAD_X
        })
        .collect();
    let total = widths.iter().sum::<f32>() + 2.0 * INSET;

    // The segments live in a child `Ui` so that its AccessKit node (the
    // container egui makes for every `Ui`) can become the radio group
    // they're announced as part of ("Contrast method, radio group").
    ui.scope(|ui| {
        a11y(ui, ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::RadioGroup);
            node.set_label(group);
        });
        let (rect, _) = ui.allocate_exact_size(egui::vec2(total, HEIGHT), egui::Sense::hover());

        ui.painter().rect_filled(rect, RADIUS, p.toolbar_bg);
        ui.painter().rect_stroke(
            rect,
            RADIUS,
            egui::Stroke::new(1.0_f32, p.border),
            egui::StrokeKind::Inside,
        );

        let base_id = ui.id().with(("segmented", group));
        let ids: Vec<egui::Id> = (0..options.len()).map(|i| base_id.with(i)).collect();
        let mut selected = options.iter().position(|(v, _)| v == value);
        let before = selected;
        let mut x = rect.left() + INSET;

        for (i, ((_, label), width)) in options.iter().zip(&widths).enumerate() {
            let segment = egui::Rect::from_min_size(
                egui::pos2(x, rect.top() + INSET),
                egui::vec2(*width, HEIGHT - 2.0 * INSET),
            );
            x += width;
            let resp = ui.interact(segment, ids[i], egui::Sense::click());
            let is_selected = selected == Some(i);
            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, is_selected, *label)
            });

            if resp.clicked() {
                selected = Some(i);
            }
            if resp.has_focus() {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        resp.id,
                        egui::EventFilter { horizontal_arrows: true, ..Default::default() },
                    )
                });
                // Only reads input inside the closure (see magnifier.rs on
                // egui's input lock); focus moves after it returns.
                let step: isize = ui.input_mut(|input| {
                    let mut d = 0;
                    if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                        d -= 1;
                    }
                    if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                        d += 1;
                    }
                    d
                });
                if step != 0 {
                    let n = options.len() as isize;
                    let j = (i as isize + step).rem_euclid(n) as usize;
                    selected = Some(j);
                    ui.memory_mut(|m| m.request_focus(ids[j]));
                }
            }

            // Drawn with this frame's selection, so a click shows at once.
            let shown_selected = selected == Some(i);
            if shown_selected {
                ui.painter().rect_filled(segment, RADIUS - 1.0, p.bg);
                ui.painter().rect_stroke(
                    segment,
                    RADIUS - 1.0,
                    egui::Stroke::new(1.5_f32, p.text),
                    egui::StrokeKind::Inside,
                );
            } else if resp.hovered() {
                ui.painter().rect_filled(segment, RADIUS - 1.0, ICON_HOVER);
            }
            let color = if shown_selected || resp.hovered() { p.text } else { p.text_secondary };
            let galley = ui
                .painter()
                .layout_no_wrap((*label).to_owned(), font(shown_selected), color);
            ui.painter()
                .galley(segment.center() - galley.size() / 2.0, galley, color);
            if resp.has_focus() {
                ui.painter().rect_stroke(
                    segment,
                    RADIUS - 1.0,
                    egui::Stroke::new(2.0, p.focus),
                    egui::StrokeKind::Outside,
                );
            }
        }

        match selected {
            Some(i) if selected != before => {
                *value = options[i].0;
                true
            }
            _ => false,
        }
    })
    .inner
}

/// A permission's state as the Permissions card shows it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PermissionState {
    Unset,
    Denied,
    Approved,
}

impl PermissionState {
    fn from_answer(answer: Option<crate::permissions::Answer>) -> Self {
        use crate::permissions::Answer;
        match answer {
            Some(Answer::Allowed) => Self::Approved,
            Some(Answer::Denied) => Self::Denied,
            Some(Answer::Unset) | None => Self::Unset,
        }
    }
}

/// One row of the Permissions card: the permission's name and its state
/// as a badge on the left, "Reset" on the right (disabled when unset,
/// with nothing to reset), and what the method does under them. Returns
/// whether "Reset" was activated.
///
/// The badge is a polite live region: it changes on its own (a pick's
/// dialog answered, a reset), and a screen reader should say so without
/// moving focus. "Reset" is named after its row for assistive tech,
/// since every row has one.
fn permission_row(
    ui: &mut egui::Ui,
    p: &Palette,
    name: &str,
    description: &str,
    state: PermissionState,
    reset_enabled: bool,
) -> bool {
    ui.scope(|ui| {
        // The description belongs to the row above it: kept close to it.
        ui.spacing_mut().item_spacing.y = 2.0;
        let reset = ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(egui::RichText::new(name).size(13.0).family(bold()).color(p.text));
            // Neutral for unset: secondary text on the toolbar grey, 5.7:1
            // (light) and 5.9:1 (dark). The other two match Pass/Fail.
            let badge = match state {
                PermissionState::Unset => tag(ui, "Unset", p.text_secondary, p.toolbar_bg),
                PermissionState::Denied => tag(ui, "Denied", egui::Color32::WHITE, FAIL_BG),
                PermissionState::Approved => tag(ui, "Approved", p.tag_fg, p.tag_bg),
            };
            a11y(ui, badge.id, |node| node.set_live(egui::accesskit::Live::Polite));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let reset = ui.add_enabled(
                    reset_enabled && state != PermissionState::Unset,
                    egui::Button::new(egui::RichText::new("Reset").size(12.0)),
                );
                let reset_name = format!("Reset {} permission", name.to_lowercase());
                a11y(ui, reset.id, |node| node.set_label(reset_name));
                reset.clicked()
            })
            .inner
        })
        .inner;
        ui.label(egui::RichText::new(description).size(12.0).color(p.text_secondary));
        reset
    })
    .inner
}

/// Card title row (flax's `.settings-card-title`): bold 14px title on the
/// left, optional trailing content on the right, then a divider.
fn card_title(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    trailing: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 24.0;
        let heading = ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .family(bold())
                .color(p.text),
        );
        // Level 2, under the window's "colza" (level 1).
        mark_heading(ui, heading.id, 2, title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
    });
    ui.add(egui::Separator::default().spacing(6.0));
}

/// A white, bordered, rounded card (flax's `.settings-card`). Its height
/// follows its content; its width fills the column.
fn card<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(p.bg)
        .stroke(egui::Stroke::new(1.0_f32, p.border))
        .corner_radius(8.0)
        .inner_margin(CARD_MARGIN)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            add_contents(ui)
        })
        .inner
}

/// State of one color card: title + format dropdown, swatch + text +
/// eyedropper, then the format's three sliders and Alpha.
struct ColorCard {
    label: &'static str,
    /// The color used for contrast. Always equal to
    /// `format.to_rgb(components).0`.
    color: Rgb,
    alpha: u8,
    format: ColorFormat,
    /// Slider positions in `format`'s model and units.
    ///
    /// Stored, not recomputed from `color` each frame, because 8-bit RGB
    /// can't hold everything the sliders can: a gray has no hue, so
    /// dragging HSL saturation to 0 and back would reset the hue to red;
    /// an OkLCh chroma dragged past sRGB would snap back to the mapped
    /// one; and every drag step would round to the nearest 8-bit color,
    /// making the other two handles jitter. They're re-derived from
    /// `color` only when the color comes from outside the sliders (pick,
    /// typing another format, format switch, swap).
    components: [f64; 3],
    /// Whether `components` lie outside sRGB (OkLCh only), in which case
    /// `color` is the gamut-mapped stand-in.
    out_of_gamut: bool,
    /// Raw text in the field, kept separate from `color` so the user can
    /// type invalid/partial values without them being clobbered every frame.
    text: String,
}

impl ColorCard {
    fn new(label: &'static str, color: Rgb) -> Self {
        let mut card = Self {
            label,
            color,
            alpha: 255,
            format: ColorFormat::Hex,
            components: [0.0; 3],
            out_of_gamut: false,
            text: String::new(),
        };
        card.set_rgb(color);
        card
    }

    fn sync_text(&mut self) {
        self.text = self.format.format(self.components);
    }

    /// Sets the color from an `Rgb`, re-deriving the sliders from it.
    fn set_rgb(&mut self, color: Rgb) {
        self.color = color;
        self.components = self.format.components(color);
        self.out_of_gamut = false;
        self.sync_text();
    }

    /// Sets the color from slider/text components, keeping them as given.
    fn set_components(&mut self, v: [f64; 3]) {
        let (color, out_of_gamut) = self.format.to_rgb(v);
        self.components = v;
        self.color = color;
        self.out_of_gamut = out_of_gamut;
    }

    /// Used by the picker: a sampled screen pixel is always opaque.
    fn set_picked_color(&mut self, color: Rgb) {
        self.alpha = 255;
        self.set_rgb(color);
    }

    /// Rail colors for slider `i`: the color with that channel swept
    /// across its range and the other two held. Two stops are exact for
    /// RGB (each channel is linear in sRGB, as the mesh interpolates);
    /// HSL and OkLCh curve, so they get enough stops to look smooth.
    fn track(&self, i: usize) -> Vec<egui::Color32> {
        let stops = match self.format {
            ColorFormat::Hex | ColorFormat::Rgb => 2,
            ColorFormat::Hsl | ColorFormat::Oklch => 25,
        };
        let max = self.format.channels()[i].max;
        (0..stops)
            .map(|k| {
                let mut v = self.components;
                v[i] = max * k as f64 / (stops - 1) as f64;
                color32(self.format.to_rgb(v).0)
            })
            .collect()
    }

    /// Draws the card contents and returns the vertical center (in screen
    /// coordinates) of the swatch/text/eyedropper row, so the caller can
    /// align the swap button with it.
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        picking: bool,
        blocked: Option<&str>,
        on_pick: impl FnOnce(),
    ) -> f32 {
        // Title + compact format dropdown.
        let before = self.format;
        let label = self.label;
        let format = &mut self.format;
        card_title(ui, p, label, |ui| {
            ui.spacing_mut().combo_width = 72.0;
            let combo = egui::ComboBox::from_id_salt((label, "format"))
                .width(72.0)
                .selected_text(
                    egui::RichText::new(format.name())
                        .size(12.0)
                        .family(bold())
                        .color(p.text),
                );
            let (combo, _) = select_box(ui, combo, format, &ColorFormat::ALL, |f| {
                egui::RichText::new(f.name()).size(12.0)
            });
            // No label is drawn next to the dropdown (the card title says
            // whose it is), so it gets its name here; egui would otherwise
            // announce only its value, "HEX".
            let name = format!("{label} color format");
            a11y(ui, combo.id, |node| node.set_label(name));
        });
        if self.format != before {
            // Re-derived from the 8-bit color, so an out-of-gamut OkLCh
            // request becomes the color actually shown, in the new format.
            self.set_rgb(self.color);
        }

        // Swatch + text field + eyedropper.
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.spacing_mut().interact_size.y = CONTROL_SIZE;

            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(CONTROL_SIZE, CONTROL_SIZE),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(rect, 6.0, egui::Color32::WHITE);
            ui.painter().rect_filled(
                rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(
                    self.color.r,
                    self.color.g,
                    self.color.b,
                    self.alpha,
                ),
            );
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(1.0_f32, p.text_secondary),
                egui::StrokeKind::Inside,
            );

            let field_width =
                (ui.available_width() - CONTROL_SIZE - ui.spacing().item_spacing.x).max(60.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .font(egui::FontId::proportional(12.0))
                    .vertical_align(egui::Align::Center)
                    .desired_width(field_width),
            );
            // Named for the same reason as the dropdown: no visible label.
            let name = format!("{} color value", self.label);
            a11y(ui, field.id, |node| node.set_label(name));
            if field.changed() {
                // Live-parse as the user types: update the color if valid,
                // leave the text alone either way. Text in the card's own
                // format sets the sliders to exactly what was typed; any
                // other format goes through RGB.
                if let Some(v) = self.format.parse(&self.text) {
                    self.set_components(v);
                } else if let Some(parsed) = parse_color(&self.text) {
                    self.color = parsed;
                    self.components = self.format.components(parsed);
                    self.out_of_gamut = false;
                }
            }
            if field.lost_focus() {
                // Normalize on blur/Enter, or restore the last valid value
                // if what's left isn't parseable.
                self.sync_text();
            }

            // Frameless eyedropper: allocate the hit target directly so no
            // button background can appear on press or focus.
            let disabled = picking || blocked.is_some();
            let (icon_rect, pick_button) = ui.allocate_exact_size(
                egui::vec2(CONTROL_SIZE, CONTROL_SIZE),
                if disabled { egui::Sense::hover() } else { egui::Sense::click() },
            );
            if pick_button.hovered() && !disabled {
                ui.painter().rect_filled(icon_rect, 6.0, ICON_HOVER);
            }
            let icon_color = if disabled {
                p.border
            } else {
                p.text_secondary
            };
            paint_eyedropper(ui.painter(), icon_rect, icon_color);
            if pick_button.has_focus() {
                ui.painter().rect_stroke(
                    icon_rect,
                    6.0,
                    egui::Stroke::new(2.0, p.focus),
                    egui::StrokeKind::Outside,
                );
            }
            let name = format!("Pick {} color from screen", self.label.to_lowercase());
            pick_button.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, !disabled, name.clone())
            });
            // The reason is also the accessible description: a disabled
            // button can't take focus, so keyboard and screen-reader users
            // would never reach the tooltip.
            if let Some(reason) = blocked {
                let reason = reason.to_owned();
                a11y(ui, pick_button.id, |node| node.set_description(reason));
            }
            if pick_button
                .on_hover_text(blocked.unwrap_or("Pick color from screen"))
                .clicked()
            {
                on_pick();
            }
        });

        ui.add_space(4.0);

        // Sliders: each rail previews the channel's range for the other
        // two channels' current values.
        let channels = self.format.channels();
        let mut v = self.components;
        let mut changed = false;
        for (i, channel) in channels.iter().enumerate() {
            let track = self.track(i);
            let mut readout = channel.readout(v[i]);
            // Said next to chroma, the one value that can push OkLCh out
            // of sRGB, so a swatch that stops changing while the handle
            // still moves doesn't look like a bug.
            if self.out_of_gamut && self.format == ColorFormat::Oklch && channel.name == "Chroma" {
                readout.push_str(" · out of sRGB");
            }
            changed |= channel_slider(ui, p, channel, self.label, &mut v[i], &readout, &track);
            ui.add_space(SLIDER_GAP);
        }
        if changed {
            self.set_components(v);
            self.sync_text();
        }

        let mut alpha = self.alpha as f64;
        let alpha_readout = format!("{}%", (alpha / 255.0 * 100.0).round());
        if channel_slider(
            ui,
            p,
            &ALPHA_CHANNEL,
            self.label,
            &mut alpha,
            &alpha_readout,
            &[p.bg, color32(self.color)],
        ) {
            self.alpha = alpha.round() as u8;
        }

        row.response.rect.center().y
    }
}

/// Which contrast method the summary section evaluates.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Wcag2,
    Apca,
}

impl Method {
    fn criteria(self) -> &'static [Criterion; 5] {
        match self {
            Self::Wcag2 => &WCAG2_CRITERIA,
            Self::Apca => &APCA_CRITERIA,
        }
    }
}

/// One criterion row: a WCAG ratio or an APCA Lc level.
struct Criterion {
    /// The threshold as shown: "4.5:1" or "Lc 75".
    threshold_text: &'static str,
    label: &'static str,
    /// Compared against the ratio (WCAG) or |Lc| (APCA).
    threshold: f64,
}

const WCAG2_CRITERIA: [Criterion; 5] = [
    Criterion { threshold_text: "3:1", label: "UI (AA)", threshold: 3.0 },
    Criterion { threshold_text: "3:1", label: "Large text (AA)", threshold: 3.0 },
    Criterion { threshold_text: "4.5:1", label: "Regular text (AA)", threshold: 4.5 },
    Criterion { threshold_text: "4.5:1", label: "Large text (AAA)", threshold: 4.5 },
    Criterion { threshold_text: "7:1", label: "Regular text (AAA)", threshold: 7.0 },
];

/// APCA's simplified ("bronze") use levels. They're minimums by use
/// case; APCA's full guidance also ties each level to font size and
/// weight, which this summary doesn't attempt.
const APCA_CRITERIA: [Criterion; 5] = [
    Criterion { threshold_text: "Lc 30", label: "Non-essential text", threshold: 30.0 },
    Criterion { threshold_text: "Lc 45", label: "Larger text, non-text UI", threshold: 45.0 },
    Criterion { threshold_text: "Lc 60", label: "Large / bold text", threshold: 60.0 },
    Criterion { threshold_text: "Lc 75", label: "Body text", threshold: 75.0 },
    Criterion { threshold_text: "Lc 90", label: "Preferred body text", threshold: 90.0 },
];

/// The screen-capture permissions' state, shown on the settings page's
/// Permissions card and used to disable picking when both are denied.
///
/// Reads of the screenshot permission (and its reset) are D-Bus
/// round-trips, run on the shared runtime and reported back over a
/// channel polled each frame, like a pick, so the window never waits.
#[derive(Default)]
struct PermissionsState {
    /// A screenshot-permission lookup (possibly after a reset) in progress.
    screenshot_task: Option<std::sync::mpsc::Receiver<crate::permissions::Lookup>>,
    /// The latest screenshot permission seen; `None` before the first
    /// lookup has come back.
    screenshot: Option<crate::permissions::Lookup>,
    /// Shows the `flatpak permission-remove` command: sandboxed, the
    /// stored answer can only be reset from outside (see `permissions.rs`).
    show_reset_command: bool,
}

struct App {
    mode: Mode,
    method: Method,
    bg: ColorCard,
    fg: ColorCard,
    /// The desktop's text-size factor (`text_scale.rs`), 1.0 = default.
    text_scale: f32,
    /// Live updates to `text_scale` from the portal watcher.
    text_scale_rx: std::sync::mpsc::Receiver<f32>,
    /// The user's own zoom on top of `text_scale` (Ctrl+/Ctrl-/Ctrl+0).
    user_zoom: f32,
    /// The zoom the window was last sized for.
    applied_zoom: f32,
    page: Page,
    prefs: crate::prefs::Prefs,
    /// Whether a screencast restore token is saved; read when the
    /// settings page opens, after a pick and after "Reset", not every
    /// frame.
    has_restore_token: bool,
    /// The settings page's Permissions card.
    permissions: PermissionsState,
    /// Widget to focus at the start of the next frame (see `navigate`).
    pending_focus: Option<egui::Id>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            method: Method::Wcag2,
            bg: ColorCard::new("Background", Rgb::new(0xFF, 0xFF, 0xFF)),
            fg: ColorCard::new("Foreground", Rgb::new(0x00, 0x00, 0x00)),
            text_scale: 1.0,
            // A receiver whose sender is already gone: never yields.
            text_scale_rx: std::sync::mpsc::channel().1,
            user_zoom: 1.0,
            applied_zoom: 1.0,
            page: Page::Main,
            prefs: crate::prefs::Prefs::default(),
            has_restore_token: false,
            permissions: PermissionsState::default(),
            pending_focus: None,
        }
    }
}

impl App {
    /// Colors as actually seen: the background over white, the foreground
    /// over that background. Returns `(bg, fg)`.
    fn effective_colors(&self) -> (Rgb, Rgb) {
        let bg = blend(self.bg.color, self.bg.alpha, Rgb::new(0xFF, 0xFF, 0xFF));
        let fg = blend(self.fg.color, self.fg.alpha, bg);
        (bg, fg)
    }

    /// Exchanges foreground and background (color and alpha). Each card
    /// keeps its own text format.
    fn swap_colors(&mut self) {
        std::mem::swap(&mut self.fg.alpha, &mut self.bg.alpha);
        if self.fg.format == self.bg.format {
            // Same model on both sides: swap the slider positions too, so
            // a swap is lossless (a gray's hue, an out-of-gamut chroma)
            // and swapping twice is a no-op.
            std::mem::swap(&mut self.fg.color, &mut self.bg.color);
            std::mem::swap(&mut self.fg.components, &mut self.bg.components);
            std::mem::swap(&mut self.fg.out_of_gamut, &mut self.bg.out_of_gamut);
            self.fg.sync_text();
            self.bg.sync_text();
        } else {
            let (fg, bg) = (self.fg.color, self.bg.color);
            self.fg.set_rgb(bg);
            self.bg.set_rgb(fg);
        }
    }

    /// Kicks off the pick flow for `target`: opens the first capture
    /// method not denied, in the order of the settings page's Permissions
    /// card, as a task on the shared runtime, and sends the resulting
    /// `ScreenSource` (plus its first frame) back over a channel. Returns
    /// immediately: nothing here blocks the calling `ui()` frame.
    ///
    /// That order is `screencast::open_best()`'s: screen sharing unless
    /// declined (asking if nothing is remembered), then a screenshot. With
    /// both denied, the eyedroppers are disabled and this isn't reached.
    fn start_picking(&mut self, target: Target) {
        let (tx, rx) = std::sync::mpsc::channel();

        match crate::runtime::shared() {
            Ok(rt) => {
                rt.spawn(async move {
                    let _ = tx.send(crate::screencast::open_best().await);
                });
            }
            Err(err) => {
                let _ = tx.send(Err(err));
            }
        }

        self.mode = Mode::Capturing(target, rx);
    }

    /// Applies the desktop text size and the user's zoom shortcuts, and
    /// resizes the window when the effective zoom has changed.
    ///
    /// egui's built-in keyboard zoom is turned off (in `main`) and redone
    /// here with the same shortcuts and steps, because its Ctrl+0 resets
    /// to 1.0, which would throw away the desktop's text size. Here the
    /// shortcuts act on a multiplier over it, and Ctrl+0 returns to the
    /// desktop's size.
    fn update_zoom(&mut self, ctx: &egui::Context) {
        use egui::gui_zoom::kb_shortcuts as kb;

        while let Ok(factor) = self.text_scale_rx.try_recv() {
            self.text_scale = factor;
        }

        // Not inside another `ctx.input` closure: each shortcut takes the
        // input lock on its own.
        let shortcut = |s: &egui::KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        if shortcut(&kb::ZOOM_RESET) {
            self.user_zoom = 1.0;
        } else {
            if shortcut(&kb::ZOOM_IN) || shortcut(&kb::ZOOM_IN_SECONDARY) {
                self.user_zoom += 0.1;
            }
            if shortcut(&kb::ZOOM_OUT) {
                self.user_zoom -= 0.1;
            }
            // Rounded so ten steps up and ten down land back on exactly 1.
            self.user_zoom = ((self.user_zoom * 10.0).round() / 10.0).clamp(0.5, 3.0);
        }

        let wanted = self.text_scale * self.user_zoom;
        if (ctx.zoom_factor() - wanted).abs() > 1e-4 {
            // Takes effect at the start of the next frame (which this
            // requests), where the check below then resizes the window.
            ctx.set_zoom_factor(wanted);
        }

        let zoom = ctx.zoom_factor();
        if (zoom - self.applied_zoom).abs() > 1e-4 {
            self.applied_zoom = zoom;
            // `InnerSize` is in points, which the zoom has just enlarged,
            // so the same `WINDOW_SIZE` now means a bigger window.
            //
            // The `Resizable` bracket is needed because a non-resizable
            // window has its min and max size pinned to its current size
            // (winit sets both on Wayland), which the compositor would
            // hold it to. Turning resizing back off afterwards re-pins
            // min = max to the new size.
            let root = egui::ViewportId::ROOT;
            ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Resizable(true));
            ctx.send_viewport_cmd_to(root, egui::ViewportCommand::InnerSize(WINDOW_SIZE.into()));
            ctx.send_viewport_cmd_to(root, egui::ViewportCommand::Resizable(false));
        }
    }

    fn card_mut(&mut self, target: Target) -> &mut ColorCard {
        match target {
            Target::Fg => &mut self.fg,
            Target::Bg => &mut self.bg,
        }
    }

    /// Header toolbar background (flax's `.toolbar-brand`): `HEADER_HEIGHT`,
    /// toolbar background, bottom border; `contents` lays out inside it,
    /// left to right, vertically centered.
    fn header_bar<R>(ui: &mut egui::Ui, p: &Palette, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let width = ui.available_width();
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, HEADER_HEIGHT));
        ui.painter().rect_filled(rect, 0.0, p.toolbar_bg);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0_f32, p.border),
        );
        ui.allocate_ui_with_layout(
            egui::vec2(width, HEADER_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            contents,
        )
        .inner
    }

    /// Main page header: logo on the left, settings button on the
    /// right. Returns the settings button's response.
    fn draw_header(ui: &mut egui::Ui, p: &Palette) -> egui::Response {
        Self::header_bar(ui, p, |ui| {
            ui.add_space(16.0);
            logo(ui, p);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                icon_button(
                    ui,
                    p,
                    Some(settings_button_id()),
                    HEADER_BUTTON_SIZE,
                    "Settings",
                    paint_gear,
                )
            })
            .inner
        })
    }

    /// Settings page header, as in flax: back arrow and title on the left;
    /// version and the GitHub mark linking to the repository on the
    /// right. Returns the back button's response.
    fn draw_settings_header(ui: &mut egui::Ui, p: &Palette) -> egui::Response {
        Self::header_bar(ui, p, |ui| {
            ui.add_space(8.0);
            let back = icon_button(
                ui,
                p,
                Some(back_button_id()),
                HEADER_BUTTON_SIZE,
                "Back",
                paint_back_arrow,
            );
            ui.add_space(8.0);
            let title = ui.label(
                egui::RichText::new("Settings")
                    .size(12.0)
                    .family(bold())
                    .color(p.text),
            );
            mark_heading(ui, title.id, 1, "Settings");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if !REPOSITORY.is_empty() {
                    github_link(ui, p, REPOSITORY);
                    ui.add_space(8.0);
                }
                ui.label(
                    egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                        .size(12.0)
                        .family(bold())
                        .color(p.text),
                );
                ui.label(egui::RichText::new("colza: ").size(12.0).color(p.text_secondary));
            });
            back
        })
    }

    /// Settings page body: Appearance and Text size on the left, Screen
    /// capture on the right. Saves and applies changes as they're made.
    fn draw_settings(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let field_label = |ui: &mut egui::Ui, text: &str| {
            ui.label(egui::RichText::new(text).size(13.0).family(bold()).color(p.text));
        };
        let body = |ui: &mut egui::Ui, text: &str| {
            ui.label(egui::RichText::new(text).size(12.0).color(p.text_secondary));
        };

        let content = ui.available_rect_before_wrap().shrink(CARD_MARGIN);
        let top_down = egui::Layout::top_down(egui::Align::Min);
        let mut changed = false;

        // Scrolls: the window has a fixed size, and the two columns are
        // taller than it once the Permissions card is in. egui scrolls the
        // focused widget into view, so Tab reaches everything too.
        ui.scope_builder(
            egui::UiBuilder::new().id_salt("settings").max_rect(content).layout(top_down),
            |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let column_width = ((ui.available_width() - COLUMN_GAP) / 2.0).max(0.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(COLUMN_GAP, COLUMN_GAP);

                        ui.vertical(|ui| {
                            ui.set_width(column_width);
                            ui.spacing_mut().item_spacing.y = COLUMN_GAP;

                            // Appearance.
                            card(ui, p, |ui| {
                                card_title(ui, p, "Appearance", |_| {});
                                field_label(ui, "Theme");
                                // Full-width and as tall as flax's <select>.
                                ui.spacing_mut().interact_size.y = 32.0;
                                let theme = &mut self.prefs.theme;
                                let combo = egui::ComboBox::from_id_salt("theme")
                                    .width(ui.available_width())
                                    .selected_text(egui::RichText::new(theme.label()).size(13.0).color(p.text));
                                let (combo, theme_changed) =
                                    select_box(ui, combo, theme, &crate::prefs::Theme::ALL, |t| {
                                        egui::RichText::new(t.label()).size(13.0)
                                    });
                                changed |= theme_changed;
                                a11y(ui, combo.id, |node| node.set_label("Theme"));
                            });

                            // Text size: just the shortcuts. The desktop's own
                            // size is applied automatically (`text_scale.rs`).
                            card(ui, p, |ui| {
                                card_title(ui, p, "Text size", |_| {});
                                egui::Frame::new()
                                    .fill(p.toolbar_bg)
                                    .stroke(egui::Stroke::new(1.0_f32, p.border))
                                    .corner_radius(6.0)
                                    .inner_margin(12.0)
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        egui::Grid::new("shortcuts")
                                            .num_columns(2)
                                            .spacing(egui::vec2(16.0, 8.0))
                                            .show(ui, |ui| {
                                                for (key, action) in [
                                                    ("+", "Larger"),
                                                    ("-", "Smaller"),
                                                    ("0", "Reset to system size"),
                                                ] {
                                                    ui.horizontal(|ui| {
                                                        ui.spacing_mut().item_spacing.x = 4.0;
                                                        keycap(ui, p, "Ctrl");
                                                        keycap(ui, p, key);
                                                    });
                                                    body(ui, action);
                                                    ui.end_row();
                                                }
                                            });
                                    });
                            });
                        });

                        ui.vertical(|ui| {
                            ui.set_width(column_width);
                            ui.spacing_mut().item_spacing.y = COLUMN_GAP;
                            self.draw_permissions(ui, p);
                        });
                    });
                });
            },
        );

        if changed {
            self.apply_theme(ui.ctx());
            self.prefs.save();
        }
    }

    /// State of the screen-sharing permission. ScreenCast stores no
    /// refusal (see `permissions.rs`): "denied" is colza's own flag for
    /// this run, "approved" a saved restore token.
    fn screencast_state(&self) -> PermissionState {
        if crate::screencast::was_declined() {
            PermissionState::Denied
        } else if self.has_restore_token {
            PermissionState::Approved
        } else {
            PermissionState::Unset
        }
    }

    fn screenshot_state(&self) -> PermissionState {
        PermissionState::from_answer(
            self.permissions.screenshot.and_then(crate::permissions::Lookup::answer),
        )
    }

    /// Whether both capture methods are denied, so no pick can succeed.
    fn picking_blocked(&self) -> bool {
        self.screencast_state() == PermissionState::Denied
            && self.screenshot_state() == PermissionState::Denied
    }

    /// The Permissions card: one row per capture method, in the order a
    /// pick tries them (see `start_picking`), with its state and "Reset".
    /// Reset only goes back to unset; the question itself comes from the
    /// next pick (see `permissions.rs` for why the two differ underneath).
    fn draw_permissions(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // No reset under a pick in flight, which may be asking right now.
        let idle = matches!(self.mode, Mode::Normal);

        card(ui, p, |ui| {
            card_title(ui, p, "Permissions", |_| {});
            ui.label(
                egui::RichText::new(
                    "colza looks at your screen only while you pick a color. It uses the \
                     first method below that isn't denied.",
                )
                .size(12.0)
                .color(p.text_secondary),
            );
            ui.add_space(4.0);

            if permission_row(
                ui,
                p,
                "Screen sharing",
                "Pick a color from a live feed of your screen.",
                self.screencast_state(),
                idle,
            ) {
                crate::screencast::forget_declined();
                crate::token_store::clear();
                self.has_restore_token = crate::token_store::exists();
            }

            ui.add(egui::Separator::default().spacing(8.0));

            let screenshot_idle = idle && self.permissions.screenshot_task.is_none();
            if permission_row(
                ui,
                p,
                "Screenshot",
                "Pick a color from a screenshot. Mouse cursor is visible.",
                self.screenshot_state(),
                screenshot_idle,
            ) {
                if crate::permissions::is_sandboxed() {
                    // colza forgets what it saw, so picking is offered
                    // again: the next pick asks if the command below was
                    // run, and comes back denied otherwise.
                    crate::permissions::record_outcome(crate::permissions::Answer::Unset);
                    self.permissions.show_reset_command = true;
                    self.refresh_screenshot_permission(false);
                } else {
                    self.refresh_screenshot_permission(true);
                }
            }

            if let Some(command) = crate::permissions::reset_command()
                .filter(|_| self.permissions.show_reset_command)
            {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "colza's sandbox can't reset this itself. To be asked again, run \
                         this command in a terminal:",
                    )
                    .size(12.0)
                    .color(p.text_secondary),
                );
                egui::Frame::new()
                    .fill(p.toolbar_bg)
                    .stroke(egui::Stroke::new(1.0_f32, p.border))
                    .corner_radius(6.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&command).monospace().size(11.0).color(p.text),
                            )
                            .selectable(true),
                        );
                    });
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size.y = 28.0;
                    if ui.button(egui::RichText::new("Copy command").size(12.0)).clicked() {
                        ui.ctx().copy_text(command);
                    }
                });
            }
        });
    }

    /// Looks the screenshot permission up again (resetting it first if
    /// `reset`) on the shared runtime; the result lands in
    /// `poll_permission_tasks`. Does nothing if a lookup is running.
    fn refresh_screenshot_permission(&mut self, reset: bool) {
        if self.permissions.screenshot_task.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        match crate::runtime::shared() {
            Ok(rt) => {
                rt.spawn(async move {
                    if reset {
                        if let Err(err) = crate::permissions::reset().await {
                            eprintln!("colza: could not reset the screenshot permission: {err}");
                        }
                    }
                    let _ = tx.send(crate::permissions::lookup().await);
                });
                self.permissions.screenshot_task = Some(rx);
            }
            Err(err) => eprintln!("colza: could not check the screenshot permission: {err}"),
        }
    }

    /// Collects a finished lookup, and starts the first one at launch:
    /// the main window needs it too, to know whether picking can work.
    fn poll_permission_tasks(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;

        if self.permissions.screenshot.is_none() && self.permissions.screenshot_task.is_none() {
            self.has_restore_token = crate::token_store::exists();
            self.refresh_screenshot_permission(false);
        }

        let state = &mut self.permissions;
        if let Some(rx) = &state.screenshot_task {
            match rx.try_recv() {
                Ok(lookup) => {
                    state.screenshot_task = None;
                    state.screenshot = Some(lookup);
                    // Kept up until a screenshot goes through: until then,
                    // the user may still need the command.
                    if lookup.answer() == Some(crate::permissions::Answer::Allowed) {
                        state.show_reset_command = false;
                    }
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
                Err(TryRecvError::Disconnected) => {
                    state.screenshot_task = None;
                    // Not left `None`, which would start a lookup per frame.
                    state.screenshot = Some(crate::permissions::Lookup::Observed(None));
                }
            }
        }
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        use crate::prefs::Theme;
        ctx.set_theme(match self.prefs.theme {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        });
    }

    /// Switches pages. Focus follows only when the switch came from the
    /// keyboard (or a screen reader): it lands on the button that leads
    /// back, so Tab order picks up from the new page, and a screen reader
    /// announces where the user ended up. After a mouse click, moving
    /// focus would just paint a focus ring nobody asked for.
    fn navigate(&mut self, ctx: &egui::Context, to: Page, from_keyboard: bool) {
        self.page = to;
        if to == Page::Settings {
            self.has_restore_token = crate::token_store::exists();
            self.refresh_screenshot_permission(false);
        }
        if from_keyboard {
            // Deferred to the next frame: the target button belongs to the
            // page that frame will draw, and doesn't exist yet in this one.
            // Focusing it now would put a focus on a node missing from
            // this frame's accessibility tree, which a screen reader can't
            // follow (AccessKit's own consumer rejects such a tree).
            self.pending_focus = Some(match to {
                Page::Settings => back_button_id(),
                Page::Main => settings_button_id(),
            });
        }
        ctx.request_repaint();
    }

    /// Icon-only "swap" button for the summary's header row. Returns true
    /// when activated by pointer or keyboard.
    fn swap_button(ui: &mut egui::Ui, p: &Palette) -> bool {
        // Same height as the segmented control beside it.
        icon_button(ui, p, None, 26.0, "Swap foreground & background", paint_swap).clicked()
    }

    /// Summary section: method switch and swap button, "Aa" preview +
    /// ratio or Lc, then the method's criteria as pass/fail tags. Returns
    /// true when the swap button was activated.
    fn draw_summary(
        ui: &mut egui::Ui,
        p: &Palette,
        bg: Rgb,
        fg: Rgb,
        method: &mut Method,
    ) -> bool {
        // Header row: the switch, then the swap button at the right. The
        // score is computed after the switch has had its say, so a click
        // shows the new method's numbers in that same frame. (A swap is
        // applied by the caller after this frame's layout, like before.)
        let mut score_text = String::new();
        let mut score = 0.0;
        let mut swap = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            segmented_control(
                ui,
                p,
                "Contrast method",
                method,
                &[(Method::Wcag2, "WCAG 2.x"), (Method::Apca, "APCA")],
            );
            (score, score_text) = match *method {
                Method::Wcag2 => {
                    let ratio = crate::color::contrast_ratio(fg, bg);
                    (ratio, format!("{}:1", format_ratio(ratio)))
                }
                // Foreground is the text: APCA is order-dependent.
                Method::Apca => {
                    let lc = crate::color::apca_contrast(fg, bg);
                    (lc.abs(), format_lc(lc))
                }
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                swap = Self::swap_button(ui, p);
            });
        });
        ui.add(egui::Separator::default().spacing(6.0));
        let criteria = method.criteria();

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 6.0, color32(bg));
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(1.0_f32, p.text_secondary),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Aa",
                egui::FontId::new(22.0, bold()),
                color32(fg),
            );
            ui.label(
                egui::RichText::new(score_text)
                    .size(32.0)
                    .family(bold())
                    .color(p.text),
            );
        });

        ui.add_space(4.0);

        for (i, criterion) in criteria.iter().enumerate() {
            if i > 0 {
                ui.add(egui::Separator::default().spacing(2.0));
            }
            Self::draw_criterion_row(ui, p, score >= criterion.threshold, criterion);
        }

        swap
    }

    fn draw_criterion_row(ui: &mut egui::Ui, p: &Palette, pass: bool, criterion: &Criterion) {
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 22.0;

            ui.allocate_ui_with_layout(
                egui::vec2(38.0, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new(criterion.threshold_text)
                            .size(11.0)
                            .color(p.text_secondary),
                    );
                },
            );
            ui.label(egui::RichText::new(criterion.label).size(12.0).color(p.text));

            // Pass/fail is conveyed by text, not just color, and is also
            // exposed to screen readers by `tag`.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if pass {
                    tag(ui, "Pass", p.tag_fg, p.tag_bg);
                } else {
                    tag(ui, "Fail", egui::Color32::WHITE, FAIL_BG);
                };
            });
        });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;

        self.update_zoom(ctx);
        self.poll_permission_tasks(ctx);
        if let Some(id) = self.pending_focus.take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }

        let p = Palette::new(ui.visuals().dark_mode);
        let picking = !matches!(self.mode, Mode::Normal);
        let blocked = self
            .picking_blocked()
            .then_some("Reset permissions in settings to enable color picking.");
        let mut pick_target: Option<Target> = None;
        let mut swap_requested = false;

        let (effective_bg, effective_fg) = self.effective_colors();
        let mut navigation: Option<(Page, bool)> = None;

        if self.page == Page::Settings {
            // Escape goes back, as it would close a dialog, unless it's
            // closing an open dropdown, which egui handles itself.
            if !egui::Popup::is_any_open(ctx)
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                navigation = Some((Page::Main, true));
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(p.toolbar_bg))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    let back = Self::draw_settings_header(ui, &p);
                    if back.clicked() {
                        navigation =
                            Some((Page::Main, !back.clicked_by(egui::PointerButton::Primary)));
                    }
                    self.draw_settings(ui, &p);
                });
        } else {
            let method = &mut self.method;
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(p.toolbar_bg))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                    let settings = Self::draw_header(ui, &p);
                    if settings.clicked() {
                        navigation = Some((
                            Page::Settings,
                            !settings.clicked_by(egui::PointerButton::Primary),
                        ));
                    }

                    let body_height = ui.available_height();
                    let top_down = egui::Layout::top_down(egui::Align::Min);

                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                        // Left: summary section, full height, separated from
                        // the colors by a vertical line (flax's sidebar border).
                        let (summary, _) = ui.allocate_exact_size(
                            egui::vec2(SUMMARY_WIDTH, body_height),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(summary, 0.0, p.bg);
                        ui.painter().vline(
                            summary.right() - 0.5,
                            summary.y_range(),
                            egui::Stroke::new(1.0_f32, p.border),
                        );
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .id_salt("summary")
                                .max_rect(summary.shrink(CARD_MARGIN))
                                .layout(top_down),
                            |ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                                if Self::draw_summary(ui, &p, effective_bg, effective_fg, method) {
                                    swap_requested = true;
                                }
                            },
                        );

                        // Right: the two color cards, each as tall as its
                        // content, with extra room after the vertical line.
                        let (colors, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), body_height),
                            egui::Sense::hover(),
                        );
                        let content = egui::Rect::from_min_max(
                            colors.min + egui::vec2(COLORS_PAD_LEFT, CARD_MARGIN),
                            colors.max - egui::vec2(CARD_MARGIN, CARD_MARGIN),
                        );
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .id_salt("colors")
                                .max_rect(content)
                                .layout(top_down),
                            |ui| {
                                let column_width =
                                    ((ui.available_width() - COLUMN_GAP) / 2.0).max(0.0);
                                let size = egui::vec2(column_width, ui.available_height());

                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = COLUMN_GAP;

                                    // Foreground.
                                    ui.allocate_ui_with_layout(size, top_down, |ui| {
                                        card(ui, &p, |ui| {
                                            self.fg.ui(ui, &p, picking, blocked, || {
                                                pick_target = Some(Target::Fg)
                                            });
                                        });
                                    });

                                    // Background.
                                    ui.allocate_ui_with_layout(size, top_down, |ui| {
                                        card(ui, &p, |ui| {
                                            self.bg.ui(ui, &p, picking, blocked, || {
                                                pick_target = Some(Target::Bg)
                                            });
                                        });
                                    });
                                });
                            },
                        );
                    });
                });
        }

        if let Some((to, from_keyboard)) = navigation {
            self.navigate(ctx, to, from_keyboard);
        }

        if swap_requested {
            self.swap_colors();
        }

        if let Some(target) = pick_target {
            self.start_picking(target);
        }

        // A pick may have just asked for (or been refused) a permission.
        let mut refresh_permissions = false;
        match &mut self.mode {
            Mode::Normal => {}

            Mode::Capturing(target, rx) => match rx.try_recv() {
                Ok(Ok((source, first_frame))) => {
                    refresh_permissions = true;
                    let mut state = MagnifierState::new(first_frame);
                    state.live = source.is_live();
                    self.mode = Mode::Magnifying(*target, Box::new(state), source);
                    ctx.request_repaint();
                }
                Ok(Err(err)) => {
                    refresh_permissions = true;
                    eprintln!("colza: screen capture failed: {err}");
                    self.mode = Mode::Normal;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(16));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    eprintln!("colza: screen capture task ended without a result");
                    self.mode = Mode::Normal;
                }
            },

            Mode::Magnifying(target, state, source) => {
                let target = *target;
                let mut should_close = false;
                let mut picked_color = None;

                if let Some(frame) = source.take_frame() {
                    state.accept_frame(frame);
                }

                ctx.show_viewport_immediate(
                    magnifier_viewport_id(),
                    egui::ViewportBuilder::default()
                        .with_fullscreen(true)
                        .with_decorations(false)
                        .with_always_on_top()
                        .with_transparent(true),
                    |ui, _class| {
                        let ctx = ui.ctx().clone();
                        let ctx = &ctx;
                        // Hidden, because `MagnifierState::draw` paints its
                        // own crosshair at `cursor_pos`. The system cursor
                        // can't follow arrow-key nudges: Wayland doesn't let
                        // clients move the pointer, so it would stay put
                        // while the picked pixel moved away from it.
                        ctx.set_cursor_icon(egui::CursorIcon::None);

                        let input = state.handle_input(ctx);

                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(ui, |ui| {
                                state.draw(ctx, ui);
                            });

                        if let Some(color) = input.picked {
                            picked_color = Some(color);
                            should_close = true;
                        } else if input.cancelled {
                            should_close = true;
                        } else if ctx.input(|i| i.viewport().close_requested()) {
                            should_close = true;
                        } else {
                            ctx.request_repaint();
                        }
                    },
                );

                if let Some(color) = picked_color {
                    self.card_mut(target).set_picked_color(color);
                }
                if should_close {
                    self.mode = Mode::Normal;
                    ctx.send_viewport_cmd_to(
                        magnifier_viewport_id(),
                        egui::ViewportCommand::Close,
                    );
                }
                ctx.request_repaint();
            }
        }

        if refresh_permissions {
            self.has_restore_token = crate::token_store::exists();
            self.refresh_screenshot_permission(false);
        }

        keep_arrows_off_focus(ctx);
    }
}

/// Stops the arrow keys from moving keyboard focus: only Tab and
/// Shift+Tab do that here, as in a native desktop form.
///
/// egui moves focus to the nearest widget in the arrow's direction unless
/// the focused widget claims the arrows through a focus-lock filter, and
/// there is no global switch for it. So this gives that claim to whatever
/// is focused, once per frame, after the widgets have drawn (the
/// filter is read at the start of the next frame). Widgets that use
/// arrows themselves (sliders, the segmented control, text fields,
/// dropdowns) still read them; buttons simply ignore them.
///
/// That includes the options of an open dropdown list: egui's spatial
/// move would leave the list for whatever lies under it (the eyedropper,
/// the sliders), so `select_box` moves between the options itself.
fn keep_arrows_off_focus(ctx: &egui::Context) {
    let Some(id) = ctx.memory(|m| m.focused()) else {
        return;
    };
    ctx.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        )
    });
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "roboto".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/Roboto-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "roboto-bold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/Roboto-Bold.ttf"
        ))),
    );

    let proportional = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    proportional.insert(0, "roboto".into());

    let mut bold_stack = vec!["roboto-bold".to_owned()];
    bold_stack.extend(
        proportional
            .iter()
            .filter(|name| *name != "roboto-bold")
            .cloned(),
    );
    fonts.families.insert(bold(), bold_stack);

    ctx.set_fonts(fonts);
}

/// egui visuals built from flax's palette. Widgets egui draws itself
/// (text field, dropdown, popup, separators) pick up borders, radii and
/// focus ring from here.
fn visuals_for(dark: bool) -> egui::Visuals {
    let p = Palette::new(dark);
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.panel_fill = p.toolbar_bg;
    visuals.window_fill = p.bg;
    visuals.window_stroke = egui::Stroke::new(1.0_f32, p.border);
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.extreme_bg_color = p.bg; // text field background
    visuals.faint_bg_color = p.toolbar_bg;
    visuals.hyperlink_color = p.link;
    visuals.selection.bg_fill = p.accent_bg;
    // Text field focus ring: 2px in flax's focus color.
    visuals.selection.stroke = egui::Stroke::new(2.0, p.focus);

    // Separators use the noninteractive stroke.
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, p.text);

    let radius = egui::CornerRadius::same(6);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
        widget.bg_fill = p.bg;
        widget.weak_bg_fill = p.bg;
        widget.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    }
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, p.text_secondary);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, p.text_secondary);
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, p.text_secondary);
    visuals.widgets.hovered.weak_bg_fill = p.toolbar_bg;
    visuals.widgets.active.weak_bg_fill = p.toolbar_bg;
    visuals.widgets.open.weak_bg_fill = p.toolbar_bg;

    visuals
}

/// Registers both themes; egui picks the one matching the system.
fn setup_style(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Light, visuals_for(false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals_for(true));
}

/// Window icon: the logo's pictogram alone (`assets/colza-icon.svg`, the
/// light/dark logos' colors without the wordmark), rendered with
/// `rsvg-convert -w 256 -h 256 colza-icon.svg -o icon-colza-256.png`.
///
/// Only used where the window itself carries its icon (X11, XWayland).
/// On Wayland, winit 0.30 drops it (`set_window_icon` is a no-op there):
/// KDE and GNOME take the icon from the `.desktop` file whose name
/// matches the window's app id, hence `APP_ID` and `data/io.github.calanoix.colza.desktop`.
const WINDOW_ICON: &[u8] = include_bytes!("../assets/icon-colza-256.png");

/// The Wayland app id, which the desktop matches to `io.github.calanoix.colza.desktop` (and
/// through it, `Icon=io.github.calanoix.colza`). eframe would default to the app name, which
/// is the same today; set here so renaming the window title can't break
/// the match. Also names the folder eframe would persist state to.
pub(crate) const APP_ID: &str = "io.github.calanoix.colza";

pub fn main() -> anyhow::Result<()> {
    // Read before the window exists, so it opens at its final size. eframe
    // creates the window before the app (with a zoom of 1), hence the size
    // scaled by hand here and the zoom set in the creator below.
    let text_scale = crate::text_scale::read_blocking();
    // Also before the window: egui can't learn it from winit on Wayland
    // (see `system_theme.rs`), and the first frame should be right.
    let system_theme = crate::system_theme::read_blocking();
    let prefs = crate::prefs::Prefs::load();

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id(APP_ID)
        .with_inner_size([WINDOW_SIZE[0] * text_scale, WINDOW_SIZE[1] * text_scale])
        .with_resizable(false)
        .with_maximize_button(false);
    match eframe::icon_data::from_png_bytes(WINDOW_ICON) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        // Only if the asset is replaced by a broken file: the window then
        // just gets the desktop's generic icon.
        Err(err) => eprintln!("colza: could not decode the window icon: {err}"),
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "colza",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            setup_style(&cc.egui_ctx);
            // Replaced by `App::update_zoom` (see there for why).
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            cc.egui_ctx.set_zoom_factor(text_scale);
            crate::system_theme::apply(&cc.egui_ctx, system_theme);
            crate::system_theme::watch(cc.egui_ctx.clone());

            let (text_scale_tx, text_scale_rx) = std::sync::mpsc::channel();
            crate::text_scale::watch(cc.egui_ctx.clone(), text_scale_tx);

            let app = App {
                text_scale,
                text_scale_rx,
                applied_zoom: text_scale,
                prefs,
                ..App::default()
            };
            app.apply_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow::anyhow!("eframe failed: {err}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lc_is_truncated_toward_zero() {
        assert_eq!(format_lc(74.96), "Lc 74.9"); // fails 75+, must not read 75
        assert_eq!(format_lc(-68.54146), "Lc 68.5"); // light on dark
        assert_eq!(format_lc(-74.96), "Lc 74.9");
        assert_eq!(format_lc(63.0), "Lc 63");
        assert_eq!(format_lc(0.0), "Lc 0");
    }

    #[test]
    fn parse_color_accepts_every_format() {
        let want = Some(Rgb::new(0x33, 0x66, 0x99));
        assert_eq!(parse_color("#336699"), want);
        assert_eq!(parse_color("336699"), want);
        assert_eq!(parse_color("rgb(51, 102, 153)"), want);
        assert_eq!(parse_color("hsl(210, 50%, 40%)"), want);
        assert_eq!(parse_color(&Rgb::new(0x33, 0x66, 0x99).to_oklch().to_css()), want);
        assert_eq!(parse_color("hsl(210, 50%)"), None);
    }

    #[test]
    fn field_text_matches_each_format() {
        let mut card = ColorCard::new("Foreground", Rgb::new(0x33, 0x66, 0x99));
        for (format, text) in [
            (ColorFormat::Hex, "#336699"),
            (ColorFormat::Rgb, "rgb(51, 102, 153)"),
            (ColorFormat::Hsl, "hsl(210, 50%, 40%)"),
        ] {
            card.format = format;
            card.set_rgb(card.color);
            assert_eq!(card.text, text);
        }
    }

    /// The reason `components` is stored: a gray's hue must survive.
    #[test]
    fn hsl_hue_survives_desaturating() {
        let mut card = ColorCard::new("Foreground", Rgb::new(0x33, 0x66, 0x99));
        card.format = ColorFormat::Hsl;
        card.set_rgb(card.color);
        let [h, _, l] = card.components;
        card.set_components([h, 0.0, l]);
        assert_eq!(card.color.r, card.color.b); // gray now
        card.set_components([h, 50.0, l]);
        assert_eq!(card.color, Rgb::new(0x33, 0x66, 0x99));
    }

    #[test]
    fn oklch_out_of_gamut_is_flagged_and_kept() {
        let mut card = ColorCard::new("Foreground", Rgb::new(0, 0, 0));
        card.format = ColorFormat::Oklch;
        card.set_components([60.0, 0.4, 145.0]);
        assert!(card.out_of_gamut);
        card.sync_text();
        assert_eq!(card.text, "oklch(60% 0.4 145)"); // what was asked for
        // The color actually used is a real sRGB color.
        assert!(card.color.to_oklch().c < 0.4);
    }

    #[test]
    fn swapping_twice_is_lossless() {
        let mut app = App::default();
        app.fg.format = ColorFormat::Oklch;
        app.bg.format = ColorFormat::Oklch;
        app.fg.set_components([60.0, 0.4, 145.0]); // out of gamut
        app.bg.set_components([95.0, 0.0, 200.0]); // gray with a hue
        let (fg, bg) = (app.fg.components, app.bg.components);
        app.swap_colors();
        assert_eq!(app.fg.components, bg);
        app.swap_colors();
        assert_eq!((app.fg.components, app.bg.components), (fg, bg));
        assert!(app.fg.out_of_gamut);
    }
}