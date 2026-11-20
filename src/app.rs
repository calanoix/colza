//! Main GUI, styled to match the flax extension (same palette, radii,
//! spacing and type scale as `panel.css`, light and dark):
//!
//! - a 36px toolbar-style header (brand name, light/dark toggle at the right);
//! - a body split by a 1px vertical line (like flax's sidebar border):
//!   - left (white, full height): contrast preview, ratio, the WCAG
//!     criteria as pass/fail tags, and a "swap foreground & background"
//!     button;
//!   - right (grey): Foreground and Background "cards", each as tall as
//!     its content (format dropdown, swatch, text field, eyedropper,
//!     R/G/B/Alpha sliders with gradient rails and value readouts).
//!
//! Picking a color opens the magnifier as a second egui viewport so the
//! main window stays visible while the loupe is up.
//!
//! The theme follows the system (light/dark) until the toggle is used.
//! The window has a fixed size (`WINDOW_SIZE`).
//!
//! Fonts: Roboto Regular and Roboto Bold are embedded from `assets/`.
//!
//! Run with: `cargo run -- gui`

use std::sync::Arc;

use eframe::egui;
use image::RgbImage;

use crate::color::Rgb;
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
const COL_GAP: f32 = 12.0;
/// Height of the header toolbar (flax: 36px).
const HEADER_HEIGHT: f32 = 36.0;
/// Height of swatch, text field and icon buttons.
const CONTROL_SIZE: f32 = 32.0;
/// Card inner margin (flax: 16px) and border width.
const CARD_MARGIN: f32 = 16.0;
/// Width of the left "contrast" section, including its right border
/// (flax: the sidebar, separated from the detail pane by a 1px line).
const LEFT_WIDTH: f32 = 260.0;
/// Fixed window size (the window is not resizable). The height fits the
/// tallest section (the left one, with the swap button under the criteria).
const WINDOW_SIZE: [f32; 2] = [840.0, 416.0];
/// Extra vertical space between two sliders (on top of the card's spacing).
const SLIDER_GAP: f32 = 6.0;
/// Space between the vertical line and the color cards.
const RIGHT_PAD_LEFT: f32 = 28.0;
/// Size of the theme toggle in the header.
const TOGGLE_SIZE: f32 = 28.0;

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

/// Which color field a pick is targeting.
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

/// How a color is written in the text field.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorFormat {
    Hex,
    Rgb,
}

impl ColorFormat {
    const ALL: [Self; 2] = [Self::Hex, Self::Rgb];

    fn name(self) -> &'static str {
        match self {
            Self::Hex => "HEX",
            Self::Rgb => "RGB",
        }
    }

    fn format(self, c: Rgb) -> String {
        match self {
            Self::Hex => c.to_hex(),
            Self::Rgb => format!("rgb({}, {}, {})", c.r, c.g, c.b),
        }
    }
}

/// Parses `#rrggbb`, `rrggbb`, or `rgb(r, g, b)`.
fn parse_color(text: &str) -> Option<Rgb> {
    let t = text.trim();

    if t.to_lowercase().starts_with("rgb") {
        let inner = t.split_once('(')?.1.strip_suffix(')')?;
        let mut parts = inner.split(',').map(|p| p.trim().parse::<u16>());
        let r = parts.next()?.ok()?;
        let g = parts.next()?.ok()?;
        let b = parts.next()?.ok()?;
        if parts.next().is_some() {
            return None; // extra component — not a valid rgb(...)
        }
        if r > 255 || g > 255 || b > 255 {
            return None;
        }
        return Some(Rgb::new(r as u8, g as u8, b as u8));
    }

    let hex = t.strip_prefix('#').unwrap_or(t);
    Rgb::from_hex(hex).ok()
}

/// `top` at opacity `alpha` composited over an opaque `under`.
fn blend(top: Rgb, alpha: u8, under: Rgb) -> Rgb {
    let a = alpha as f32 / 255.0;
    let mix = |t: u8, u: u8| (t as f32 * a + u as f32 * (1.0 - a)).round() as u8;
    Rgb::new(mix(top.r, under.r), mix(top.g, under.g), mix(top.b, under.b))
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

/// Round-handle slider painted by hand: a gradient rail from `track.0` to
/// `track.1` (so the rail previews what the channel does), a handle with an
/// accent outline, a value readout, and pointer + keyboard support (arrows
/// ±1, Shift+arrows ±10) with an accessible name/value. Returns true if
/// `value` changed.
fn channel_slider(
    ui: &mut egui::Ui,
    p: &Palette,
    name: &str,
    a11y_prefix: &str,
    value: &mut u8,
    track: (egui::Color32, egui::Color32),
    as_percent: bool,
) -> bool {
    const R: f32 = 9.0;

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;

        // Name on the left, current value on the right.
        let readout = if as_percent {
            format!("{}%", (*value as f32 / 255.0 * 100.0).round() as u32)
        } else {
            value.to_string()
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(name).size(11.0).color(p.text_secondary));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(readout).size(11.0).color(p.text));
            });
        });

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 2.0 * R),
            egui::Sense::click_and_drag(),
        );
        let label = format!("{a11y_prefix} {name}");
        resp.widget_info(|| egui::WidgetInfo::slider(true, *value as f64, label.clone()));

        let x0 = rect.left() + R;
        let x1 = rect.right() - R;
        let mut changed = false;

        if resp.is_pointer_button_down_on() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let v = (t * 255.0).round() as u8;
                if v != *value {
                    *value = v;
                    changed = true;
                }
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
            let delta: i32 = ui.input_mut(|i| {
                let mut d = 0;
                for (key, sign) in [
                    (egui::Key::ArrowLeft, -1),
                    (egui::Key::ArrowDown, -1),
                    (egui::Key::ArrowRight, 1),
                    (egui::Key::ArrowUp, 1),
                ] {
                    if i.consume_key(egui::Modifiers::SHIFT, key) {
                        d += 10 * sign;
                    } else if i.consume_key(egui::Modifiers::NONE, key) {
                        d += sign;
                    }
                }
                d
            });
            if delta != 0 {
                let v = (*value as i32 + delta).clamp(0, 255) as u8;
                if v != *value {
                    *value = v;
                    changed = true;
                }
            }
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        let cx = x0 + (*value as f32 / 255.0) * (x1 - x0);

        // Gradient rail. It stops at the handle's travel limits, so its
        // square ends are always hidden under the handle.
        let rail = egui::Rect::from_min_max(egui::pos2(x0, cy - 3.0), egui::pos2(x1, cy + 3.0));
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(rail.left_top(), track.0);
        mesh.colored_vertex(rail.right_top(), track.1);
        mesh.colored_vertex(rail.right_bottom(), track.1);
        mesh.colored_vertex(rail.left_bottom(), track.0);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
        painter.rect_stroke(
            rail,
            0.0,
            egui::Stroke::new(1.0_f32, p.text_secondary),
            egui::StrokeKind::Outside,
        );

        let center = egui::pos2(cx, cy);
        painter.circle_filled(center, R - 2.0, p.bg);
        painter.circle_stroke(center, R - 2.0, egui::Stroke::new(2.0, p.accent));
        if resp.has_focus() {
            painter.circle_stroke(center, R + 1.0, egui::Stroke::new(2.0, p.focus));
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
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') { i += 1; }
        if i >= bytes.len() { break; }
        if bytes[i].is_ascii_alphabetic() { command = bytes[i]; i += 1; }
        match command {
            b'M' | b'L' => {
                let point = egui::pos2(number(bytes, &mut i), number(bytes, &mut i));
                if command == b'M' {
                    if !points.is_empty() { polygons.push(std::mem::take(&mut points)); }
                    start = point;
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
                for step in 1..=8 {
                    let t = step as f32 / 8.0;
                    let u = 1.0 - t;
                    points.push(egui::pos2(
                        u*u*u*p0.x + 3.0*u*u*t*p1.x + 3.0*u*t*t*p2.x + t*t*t*p3.x,
                        u*u*u*p0.y + 3.0*u*u*t*p1.y + 3.0*u*t*t*p2.y + t*t*t*p3.y,
                    ));
                }
                current = p3;
            }
            b'z' | b'Z' => {
                current = start;
                if !points.is_empty() { polygons.push(std::mem::take(&mut points)); }
                command = b' ';
            }
            _ => break,
        }
    }
    if !points.is_empty() { polygons.push(points); }
    polygons
}

fn paint_eyedropper(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    // Render the SVG path 18 px wide, centered inside the hit target.
    // The visible path spans approximately 526 units in the 640 x 640 viewBox.
    let scale = 18.0 / 526.0;
    let origin = rect.center() - egui::vec2(323.0 * scale, 315.0 * scale);
    for polygon in eyedropper_svg_polygons() {
        let points = polygon.into_iter()
            .map(|p| origin + egui::vec2(p.x * scale, p.y * scale))
            .collect();
        painter.add(egui::Shape::Path(egui::epaint::PathShape {
            points,
            closed: true,
            fill: color,
            stroke: egui::Stroke::NONE.into(),
        }));
    }
}

/// A double arrow (⇄) drawn with strokes, like the eyedropper: no icon
/// font needed.
fn paint_swap(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let c = rect.center();
    let p = |x: f32, y: f32| c + egui::vec2(x, y);
    let s = egui::Stroke::new(2.0, color);
    // Top arrow, pointing right.
    painter.line_segment([p(-9.0, -4.0), p(9.0, -4.0)], s);
    painter.line_segment([p(5.0, -8.0), p(9.0, -4.0)], s);
    painter.line_segment([p(5.0, 0.0), p(9.0, -4.0)], s);
    // Bottom arrow, pointing left.
    painter.line_segment([p(9.0, 4.0), p(-9.0, 4.0)], s);
    painter.line_segment([p(-5.0, 0.0), p(-9.0, 4.0)], s);
    painter.line_segment([p(-5.0, 8.0), p(-9.0, 4.0)], s);
}

/// Sun icon (shown in dark mode: click to go light), drawn with strokes.
fn paint_sun(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let c = rect.center();
    painter.circle_filled(c, 4.5, color);
    let stroke = egui::Stroke::new(2.0, color);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::FRAC_PI_4;
        let dir = egui::vec2(a.cos(), a.sin());
        painter.line_segment([c + dir * 7.5, c + dir * 10.0], stroke);
    }
}

/// Crescent moon icon (shown in light mode: click to go dark). The
/// crescent is the outer circle minus an offset inner circle; it is built
/// as a triangle strip between the two arcs (egui can't fill concave paths
/// reliably) plus a thin outline for smooth edges.
fn paint_moon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    const R: f32 = 8.5; // outer radius
    const INNER_R: f32 = 7.2; // inner (cut-out) radius
    const STEPS: usize = 24;

    let c = rect.center();
    // The inner circle is offset toward the upper right.
    let u = egui::vec2(0.8, -0.6);
    let d = 6.0_f32;
    let inner_c = c + u * d;

    // The crescent's tips are where the two circles cross; `a` is the
    // distance from the outer center to the chord joining them.
    let a = (R * R - INNER_R * INNER_R + d * d) / (2.0 * d);
    // Directions are measured from -u, the crescent's "thick" side.
    let m = (-u.y).atan2(-u.x);
    let phi = (-a / R).clamp(-1.0, 1.0).acos();
    let psi = ((d - a) / INNER_R).clamp(-1.0, 1.0).acos();

    let mut outer = Vec::with_capacity(STEPS + 1);
    let mut inner = Vec::with_capacity(STEPS + 1);
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let ao = m - phi + 2.0 * phi * t;
        let ai = m - psi + 2.0 * psi * t;
        outer.push(c + egui::vec2(ao.cos(), ao.sin()) * R);
        inner.push(inner_c + egui::vec2(ai.cos(), ai.sin()) * INNER_R);
    }

    let mut mesh = egui::Mesh::default();
    for i in 0..=STEPS {
        mesh.colored_vertex(outer[i], color);
        mesh.colored_vertex(inner[i], color);
    }
    for i in 0..STEPS as u32 {
        let o0 = 2 * i;
        let i0 = 2 * i + 1;
        let o1 = 2 * i + 2;
        let i1 = 2 * i + 3;
        mesh.add_triangle(o0, i0, o1);
        mesh.add_triangle(i0, i1, o1);
    }
    painter.add(egui::Shape::mesh(mesh));

    let mut outline = outer;
    outline.extend(inner.into_iter().rev());
    painter.add(egui::Shape::closed_line(
        outline,
        egui::Stroke::new(1.0_f32, color),
    ));
}

/// Small uppercase tag, like flax's `.best-practice-tag`. Exposes its
/// text to screen readers.
fn tag(ui: &mut egui::Ui, text: &str, fg: egui::Color32, bg: egui::Color32) {
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
        ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .family(bold())
                .color(p.text),
        );
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

/// One color card: title + format dropdown, swatch + text + eyedropper,
/// then R/G/B/Alpha sliders.
struct ColorRowState {
    label: &'static str,
    color: Rgb,
    alpha: u8,
    format: ColorFormat,
    /// Raw text in the field, kept separate from `color` so the user can
    /// type invalid/partial values without them being clobbered every frame.
    text: String,
}

impl ColorRowState {
    fn new(label: &'static str, color: Rgb) -> Self {
        Self {
            label,
            color,
            alpha: 255,
            format: ColorFormat::Hex,
            text: ColorFormat::Hex.format(color),
        }
    }

    fn sync_text(&mut self) {
        self.text = self.format.format(self.color);
    }

    /// Used by the picker: a sampled screen pixel is always opaque.
    fn set_color(&mut self, color: Rgb) {
        self.color = color;
        self.alpha = 255;
        self.sync_text();
    }

    /// Draws the card contents and returns the vertical center (in screen
    /// coordinates) of the swatch/text/eyedropper row, so the caller can
    /// align the swap button with it.
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        picking: bool,
        on_pick: impl FnOnce(),
    ) -> f32 {
        // Title + compact format dropdown.
        let before = self.format;
        let label = self.label;
        let format = &mut self.format;
        card_title(ui, p, label, |ui| {
            ui.spacing_mut().combo_width = 72.0;
            egui::ComboBox::from_id_salt((label, "format"))
                .width(72.0)
                .selected_text(
                    egui::RichText::new(format.name())
                        .size(12.0)
                        .family(bold())
                        .color(p.text),
                )
                .show_ui(ui, |ui| {
                    for f in ColorFormat::ALL {
                        ui.selectable_value(
                            format,
                            f,
                            egui::RichText::new(f.name()).size(12.0),
                        );
                    }
                });
        });
        if self.format != before {
            self.sync_text();
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
            if field.changed() {
                // Live-parse as the user types: update color if valid,
                // leave the text alone either way.
                if let Some(parsed) = parse_color(&self.text) {
                    self.color = parsed;
                }
            }
            if field.lost_focus() {
                // Normalize on blur/Enter, or restore the last valid value
                // if what's left isn't parseable.
                self.sync_text();
            }

            // Frameless eyedropper: allocate the hit target directly so no
            // button background can appear on press or focus.
            let (icon_rect, btn) = ui.allocate_exact_size(
                egui::vec2(CONTROL_SIZE, CONTROL_SIZE),
                if picking { egui::Sense::hover() } else { egui::Sense::click() },
            );
            if btn.hovered() && !picking {
                ui.painter().rect_filled(icon_rect, 6.0, ICON_HOVER);
            }
            let icon_color = if picking {
                p.border
            } else {
                p.text_secondary
            };
            paint_eyedropper(ui.painter(), icon_rect, icon_color);
            if btn.has_focus() {
                ui.painter().rect_stroke(
                    icon_rect,
                    6.0,
                    egui::Stroke::new(2.0, p.focus),
                    egui::StrokeKind::Outside,
                );
            }
            let name = format!("Pick {} color from screen", self.label.to_lowercase());
            btn.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, !picking, name.clone())
            });
            if btn.on_hover_text("Pick color from screen").clicked() {
                on_pick();
            }
        });

        ui.add_space(4.0);

        // Sliders: each rail previews the channel's range for the other
        // two channels' current values.
        let (r, g, b) = (self.color.r, self.color.g, self.color.b);
        let c = egui::Color32::from_rgb;
        let mut changed = false;
        changed |= channel_slider(
            ui, p, "Red", self.label, &mut self.color.r,
            (c(0, g, b), c(255, g, b)), false,
        );
        ui.add_space(SLIDER_GAP);
        changed |= channel_slider(
            ui, p, "Green", self.label, &mut self.color.g,
            (c(r, 0, b), c(r, 255, b)), false,
        );
        ui.add_space(SLIDER_GAP);
        changed |= channel_slider(
            ui, p, "Blue", self.label, &mut self.color.b,
            (c(r, g, 0), c(r, g, 255)), false,
        );
        if changed {
            self.sync_text();
        }
        ui.add_space(SLIDER_GAP);
        channel_slider(
            ui, p, "Alpha", self.label, &mut self.alpha,
            (p.bg, color32(self.color)), true,
        );

        row.response.rect.center().y
    }
}

/// One WCAG criterion row.
struct Criterion {
    ratio_text: &'static str,
    label: &'static str,
    threshold: f64,
}

const CRITERIA: [Criterion; 5] = [
    Criterion { ratio_text: "3:1", label: "UI (AA)", threshold: 3.0 },
    Criterion { ratio_text: "3:1", label: "Large text (AA)", threshold: 3.0 },
    Criterion { ratio_text: "4.5:1", label: "Regular text (AA)", threshold: 4.5 },
    Criterion { ratio_text: "4.5:1", label: "Large text (AAA)", threshold: 4.5 },
    Criterion { ratio_text: "7:1", label: "Regular text (AAA)", threshold: 7.0 },
];

struct App {
    mode: Mode,
    bg: ColorRowState,
    fg: ColorRowState,
}

impl Default for App {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            bg: ColorRowState::new("Background", Rgb::new(0xFF, 0xFF, 0xFF)),
            fg: ColorRowState::new("Foreground", Rgb::new(0x00, 0x00, 0x00)),
        }
    }
}

impl App {
    /// Colors as actually seen: the background over white, the foreground
    /// over that background.
    fn effective(&self) -> (Rgb, Rgb) {
        let bg = blend(self.bg.color, self.bg.alpha, Rgb::new(0xFF, 0xFF, 0xFF));
        let fg = blend(self.fg.color, self.fg.alpha, bg);
        (bg, fg)
    }

    /// Exchanges foreground and background (color and alpha). Each row
    /// keeps its own text format.
    fn swap_colors(&mut self) {
        std::mem::swap(&mut self.fg.color, &mut self.bg.color);
        std::mem::swap(&mut self.fg.alpha, &mut self.bg.alpha);
        self.fg.sync_text();
        self.bg.sync_text();
    }

    /// Kicks off the pick flow for `target`: spawns `screencast::open_best()`
    /// as a task on the shared runtime and sends the resulting
    /// `ScreenSource` (plus its first frame) back over a channel. Returns
    /// immediately — nothing here blocks the calling `ui()` frame.
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

    fn row_mut(&mut self, target: Target) -> &mut ColorRowState {
        match target {
            Target::Fg => &mut self.fg,
            Target::Bg => &mut self.bg,
        }
    }

    /// Header toolbar (flax's `.toolbar-brand`): 36px, toolbar background,
    /// bottom border, brand name on the left and a muted subtitle on the
    /// right (flax shows "WCAG 2.2 AA" there).
    /// Returns true when the theme toggle (top right) was activated.
    fn draw_header(ui: &mut egui::Ui, p: &Palette, dark: bool) -> bool {
        let width = ui.available_width();
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, HEADER_HEIGHT));
        ui.painter().rect_filled(rect, 0.0, p.toolbar_bg);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0_f32, p.border),
        );

        let mut toggled = false;
        ui.allocate_ui_with_layout(
            egui::vec2(width, HEADER_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("colza")
                        .size(12.0)
                        .family(bold())
                        .color(p.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    toggled = Self::theme_toggle(ui, p, dark);
                });
            },
        );
        toggled
    }

    /// Full-width "swap" button for the left section: swap icon + text,
    /// bordered like the text fields. Returns true when activated by
    /// pointer or keyboard.
    fn swap_button(ui: &mut egui::Ui, p: &Palette) -> bool {
        const LABEL: &str = "Swap foreground & background";
        const ICON: f32 = 20.0;
        const GAP: f32 = 8.0;

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), CONTROL_SIZE),
            egui::Sense::click(),
        );
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, LABEL)
        });

        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, p.toolbar_bg);
        }
        let border = if resp.hovered() { p.text_secondary } else { p.border };
        ui.painter().rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0_f32, border),
            egui::StrokeKind::Inside,
        );

        // Icon + text, centered as a group.
        let galley = ui.painter().layout_no_wrap(
            LABEL.to_owned(),
            egui::FontId::proportional(12.0),
            p.text,
        );
        let group_w = ICON + GAP + galley.size().x;
        let x0 = rect.center().x - group_w / 2.0;
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(x0 + ICON / 2.0, rect.center().y),
            egui::vec2(ICON, ICON),
        );
        paint_swap(ui.painter(), icon_rect, p.text_secondary);
        ui.painter().galley(
            egui::pos2(x0 + ICON + GAP, rect.center().y - galley.size().y / 2.0),
            galley,
            p.text,
        );

        if resp.has_focus() {
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(2.0, p.focus),
                egui::StrokeKind::Outside,
            );
        }

        resp.clicked()
    }

    /// Light/dark toggle (header, top right). Shows the icon of the mode it
    /// switches *to*. Returns true when activated by pointer or keyboard.
    fn theme_toggle(ui: &mut egui::Ui, p: &Palette, dark: bool) -> bool {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(TOGGLE_SIZE, TOGGLE_SIZE),
            egui::Sense::click(),
        );
        let label = if dark {
            "Switch to light mode"
        } else {
            "Switch to dark mode"
        };
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));

        if resp.hovered() {
            ui.painter().rect_filled(rect, 6.0, ICON_HOVER);
        }
        if dark {
            paint_sun(ui.painter(), rect, p.text_secondary);
        } else {
            paint_moon(ui.painter(), rect, p.text_secondary);
        }
        if resp.has_focus() {
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(2.0, p.focus),
                egui::StrokeKind::Outside,
            );
        }

        resp.on_hover_text(label).clicked()
    }

    /// Left section: "Aa" preview + ratio, then the criteria as pass/fail tags.
    fn draw_summary(ui: &mut egui::Ui, p: &Palette, bg: Rgb, fg: Rgb, ratio: f64) {
        let passed = CRITERIA.iter().filter(|c| ratio >= c.threshold).count();
        card_title(ui, p, "WCAG 2.X", |ui| {
            ui.label(
                egui::RichText::new(format!("{passed}/{} passed", CRITERIA.len()))
                    .size(11.0)
                    .color(p.text_secondary),
            );
        });

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
                egui::RichText::new(format!("{}:1", format_ratio(ratio)))
                    .size(32.0)
                    .family(bold())
                    .color(p.text),
            );
        });

        ui.add_space(4.0);

        for (i, c) in CRITERIA.iter().enumerate() {
            if i > 0 {
                ui.add(egui::Separator::default().spacing(2.0));
            }
            Self::criterion_row(ui, p, ratio >= c.threshold, c);
        }
    }

    fn criterion_row(ui: &mut egui::Ui, p: &Palette, pass: bool, c: &Criterion) {
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 22.0;

            ui.allocate_ui_with_layout(
                egui::vec2(38.0, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new(c.ratio_text)
                            .size(11.0)
                            .color(p.text_secondary),
                    );
                },
            );
            ui.label(egui::RichText::new(c.label).size(12.0).color(p.text));

            // Pass/fail is conveyed by text, not just color, and is also
            // exposed to screen readers by `tag`.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if pass {
                    tag(ui, "Pass", p.tag_fg, p.tag_bg);
                } else {
                    tag(ui, "Fail", egui::Color32::WHITE, FAIL_BG);
                }
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

        let p = Palette::new(ui.visuals().dark_mode);
        let picking = !matches!(self.mode, Mode::Normal);
        let mut pick_target: Option<Target> = None;
        let mut swap_requested = false;

        let (bg_eff, fg_eff) = self.effective();
        let ratio = crate::color::contrast_ratio(fg_eff, bg_eff);

        let dark = ui.visuals().dark_mode;
        let mut toggle_theme = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.toolbar_bg))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                toggle_theme = Self::draw_header(ui, &p, dark);

                let body_h = ui.available_height();
                let top_down = egui::Layout::top_down(egui::Align::Min);

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                    // Left: contrast section, full height, separated from
                    // the colors by a vertical line (flax's sidebar border).
                    let (left, _) = ui.allocate_exact_size(
                        egui::vec2(LEFT_WIDTH, body_h),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(left, 0.0, p.bg);
                    ui.painter().vline(
                        left.right() - 0.5,
                        left.y_range(),
                        egui::Stroke::new(1.0_f32, p.border),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("summary")
                            .max_rect(left.shrink(CARD_MARGIN))
                            .layout(top_down),
                        |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                            Self::draw_summary(ui, &p, bg_eff, fg_eff, ratio);
                            ui.add_space(20.0);
                            if Self::swap_button(ui, &p) {
                                swap_requested = true;
                            }
                        },
                    );

                    // Right: the two color cards, each as tall as its
                    // content, with extra room after the vertical line.
                    let (right, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), body_h),
                        egui::Sense::hover(),
                    );
                    let content = egui::Rect::from_min_max(
                        right.min + egui::vec2(RIGHT_PAD_LEFT, CARD_MARGIN),
                        right.max - egui::vec2(CARD_MARGIN, CARD_MARGIN),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt("colors")
                            .max_rect(content)
                            .layout(top_down),
                        |ui| {
                            let col_w = ((ui.available_width() - COL_GAP) / 2.0).max(0.0);
                            let size = egui::vec2(col_w, ui.available_height());

                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = COL_GAP;

                                // Foreground.
                                ui.allocate_ui_with_layout(size, top_down, |ui| {
                                    card(ui, &p, |ui| {
                                        self.fg.ui(ui, &p, picking, || {
                                            pick_target = Some(Target::Fg)
                                        });
                                    });
                                });

                                // Background.
                                ui.allocate_ui_with_layout(size, top_down, |ui| {
                                    card(ui, &p, |ui| {
                                        self.bg.ui(ui, &p, picking, || {
                                            pick_target = Some(Target::Bg)
                                        });
                                    });
                                });
                            });
                        },
                    );
                });
            });

        if toggle_theme {
            ctx.set_theme(if dark {
                egui::Theme::Light
            } else {
                egui::Theme::Dark
            });
            ctx.request_repaint();
        }

        if swap_requested {
            self.swap_colors();
        }

        if let Some(target) = pick_target {
            self.start_picking(target);
        }

        match &mut self.mode {
            Mode::Normal => {}

            Mode::Capturing(target, rx) => match rx.try_recv() {
                Ok(Ok((source, first_frame))) => {
                    let mut state = MagnifierState::new(first_frame);
                    state.live = source.is_live();
                    self.mode = Mode::Magnifying(*target, Box::new(state), source);
                    ctx.request_repaint();
                }
                Ok(Err(err)) => {
                    eprintln!("screen capture failed: {err}");
                    self.mode = Mode::Normal;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(16));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    eprintln!("screen capture task ended without a result");
                    self.mode = Mode::Normal;
                }
            },

            Mode::Magnifying(target, state, capture) => {
                let target = *target;
                let mut should_close = false;
                let mut picked_color = None;

                if let Some(frame) = capture.take_frame() {
                    state.screenshot = frame;
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
                        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);

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
                    self.row_mut(target).set_color(color);
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
    }
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
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    v.panel_fill = p.toolbar_bg;
    v.window_fill = p.bg;
    v.window_stroke = egui::Stroke::new(1.0_f32, p.border);
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.extreme_bg_color = p.bg; // text field background
    v.faint_bg_color = p.toolbar_bg;
    v.hyperlink_color = p.link;
    v.selection.bg_fill = p.accent_bg;
    // Text field focus ring: 2px in flax's focus color.
    v.selection.stroke = egui::Stroke::new(2.0, p.focus);

    // Separators use the noninteractive stroke.
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, p.text);

    let r = egui::CornerRadius::same(6);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
        w.bg_fill = p.bg;
        w.weak_bg_fill = p.bg;
        w.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    }
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, p.border);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, p.text_secondary);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, p.text_secondary);
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, p.text_secondary);
    v.widgets.hovered.weak_bg_fill = p.toolbar_bg;
    v.widgets.active.weak_bg_fill = p.toolbar_bg;
    v.widgets.open.weak_bg_fill = p.toolbar_bg;

    v
}

/// Registers both themes; egui picks the one matching the system.
fn setup_style(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Light, visuals_for(false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals_for(true));
}

pub fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            .with_maximize_button(false),
        ..Default::default()
    };

    eframe::run_native(
        "colza",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            setup_style(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
    .map_err(|err| anyhow::anyhow!("eframe failed: {err}"))?;

    Ok(())
}