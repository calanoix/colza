//! Main GUI, laid out in three columns:
//!
//! - left: preview ("Aa" + contrast ratio) and the WCAG criteria list;
//! - middle: Foreground (format dropdown, swatch, text field, eyedropper,
//!   R/G/B/Alpha sliders);
//! - right: the same for Background, with a swap button between the two
//!   color columns.
//!
//! Picking a color opens the magnifier as a second egui viewport so the
//! main window stays visible while the loupe is up.
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

const MUTED: egui::Color32 = egui::Color32::from_gray(0x55);
const INK: egui::Color32 = egui::Color32::from_gray(0x11);
const PANEL_GRAY: egui::Color32 = egui::Color32::from_rgb(0xF1, 0xF2, 0xF4);
const FAIL_RED: egui::Color32 = egui::Color32::from_rgb(0xB4, 0x2B, 0x0A);
const FOCUS_BLUE: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x73, 0xE8);

/// Size of the swap button, and of the slot reserved for it between the
/// two color columns.
const SWAP_SIZE: f32 = 38.0;
/// Horizontal gap between the layout columns.
const COL_GAP: f32 = 24.0;

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

/// Round-handle slider painted by hand (black rail, white handle), with
/// pointer + keyboard support (arrows ±1, Shift+arrows ±10) and an
/// accessible name/value. Returns true if `value` changed.
fn channel_slider(ui: &mut egui::Ui, name: &str, a11y_prefix: &str, value: &mut u8) -> bool {
    const R: f32 = 12.0;

    ui.label(egui::RichText::new(name).size(16.0).color(MUTED));
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
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), cy - 3.0),
            egui::pos2(rect.right(), cy + 3.0),
        ),
        3.0,
        INK,
    );
    let center = egui::pos2(cx, cy);
    painter.circle_filled(center, R - 1.0, egui::Color32::WHITE);
    painter.circle_stroke(center, R - 1.0, egui::Stroke::new(2.0, INK));
    if resp.has_focus() {
        painter.circle_stroke(center, R + 1.5, egui::Stroke::new(2.0, FOCUS_BLUE));
    }

    changed
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
    // Render the SVG path at 20 px wide, centered inside the 38 x 38 px hit target.
    // The visible path spans approximately 526 units in the 640 x 640 viewBox.
    let scale = 20.0 / 526.0;
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
    // Draw a 20 px icon centered in the larger clickable area.
    let c = rect.center();
    let scale = 20.0 / 18.0;
    let p = |x: f32, y: f32| c + egui::vec2(x * scale, y * scale);
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

/// One color column: format dropdown, label, swatch + text + eyedropper,
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

    /// Draws the column and returns the vertical center (in screen
    /// coordinates) of the swatch/text/eyedropper row, so the caller can
    /// align the swap button with it.
    fn ui(&mut self, ui: &mut egui::Ui, picking: bool, on_pick: impl FnOnce()) -> f32 {
        // Compact format dropdown, transparent in every interaction state.
        let before = self.format;
        ui.scope(|ui| {
            let visuals = &mut ui.style_mut().visuals;
            for widget in [&mut visuals.widgets.inactive, &mut visuals.widgets.hovered, &mut visuals.widgets.active] {
                // Keep the outline, but remove the grey fill.
                widget.bg_fill = egui::Color32::TRANSPARENT;
                widget.weak_bg_fill = egui::Color32::TRANSPARENT;
            }
            ui.spacing_mut().button_padding = egui::vec2(3.0, 2.0);
            ui.spacing_mut().combo_width = 88.0;
            egui::ComboBox::from_id_salt((self.label, "format"))
                .width(88.0)
                .selected_text(egui::RichText::new(self.format.name()).size(16.0).strong())
                .show_ui(ui, |ui| {
                    for f in ColorFormat::ALL {
                        ui.selectable_value(&mut self.format, f, f.name());
                    }
                });
        });
        if self.format != before {
            self.sync_text();
        }

        ui.label(
            egui::RichText::new(self.label)
                .size(18.0)
                .family(bold())
                .color(MUTED),
        );

        // Swatch + text field + eyedropper.
        let row = ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
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
                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x88)),
                egui::StrokeKind::Inside,
            );

            let field_width =
                (ui.available_width() - 38.0 - ui.spacing().item_spacing.x).max(60.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .font(egui::FontId::proportional(16.0))
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
            // button background can appear on hover, press, or focus.
            let (icon_rect, btn) = ui.allocate_exact_size(
                egui::vec2(38.0, 38.0),
                if picking { egui::Sense::hover() } else { egui::Sense::click() },
            );
            let icon_color = egui::Color32::from_rgb(0x5F, 0x63, 0x68);
            if btn.hovered() && !picking {
                ui.painter().rect_filled(
                    icon_rect,
                    6.0,
                    egui::Color32::from_rgb(0xF1, 0xF3, 0xF4),
                );
            }
            paint_eyedropper(ui.painter(), icon_rect, icon_color);
            if btn.has_focus() {
                ui.painter().rect_stroke(
                    icon_rect,
                    6.0,
                    egui::Stroke::new(2.0, FOCUS_BLUE),
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

        // Sliders.
        let mut changed = false;
        changed |= channel_slider(ui, "Red", self.label, &mut self.color.r);
        changed |= channel_slider(ui, "Green", self.label, &mut self.color.g);
        changed |= channel_slider(ui, "Blue", self.label, &mut self.color.b);
        if changed {
            self.sync_text();
        }
        channel_slider(ui, "Alpha", self.label, &mut self.alpha);

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

    /// Swap button, drawn by hand into `rect` (which sits in the gap
    /// between the two color columns). Returns true when activated by
    /// pointer or keyboard.
    fn swap_button(ui: &mut egui::Ui, rect: egui::Rect) -> bool {
        let resp = ui.interact(rect, ui.id().with("swap_colors"), egui::Sense::click());
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                "Swap foreground and background colors",
            )
        });

        let icon_color = egui::Color32::from_rgb(0x5F, 0x63, 0x68);
        if resp.hovered() {
            ui.painter().rect_filled(
                rect,
                6.0,
                egui::Color32::from_rgb(0xF1, 0xF3, 0xF4),
            );
        }
        paint_swap(ui.painter(), rect, icon_color);
        if resp.has_focus() {
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(2.0, FOCUS_BLUE),
                egui::StrokeKind::Outside,
            );
        }

        resp.on_hover_text("Swap colors").clicked()
    }

    /// Left column: "Aa" preview + ratio, then the criteria box.
    fn draw_summary(ui: &mut egui::Ui, bg: Rgb, fg: Rgb, ratio: f64) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 6.0, egui::Color32::from_rgb(bg.r, bg.g, bg.b));
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x88)),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Aa",
                egui::FontId::proportional(24.0),
                egui::Color32::from_rgb(fg.r, fg.g, fg.b),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("{}:1", format_ratio(ratio)))
                    .size(44.0)
                    .strong()
                    .color(INK),
            );
        });

        ui.add_space(8.0);

        egui::Frame::new()
            .fill(PANEL_GRAY)
            .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0xDD)))
            .corner_radius(10.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for c in &CRITERIA {
                    Self::criterion_row(ui, ratio >= c.threshold, c);
                }
            });
    }

    fn criterion_row(ui: &mut egui::Ui, pass: bool, c: &Criterion) {
        ui.horizontal(|ui| {
            // Pass/fail is conveyed by shape (check vs cross), not just
            // color, and is also exposed as text to screen readers.
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Label,
                    true,
                    if pass { "Pass" } else { "Fail" },
                )
            });
            let ctr = rect.center();
            let p = |x: f32, y: f32| ctr + egui::vec2(x, y);
            if pass {
                ui.painter().add(egui::Shape::line(
                    vec![p(-7.0, 0.0), p(-2.0, 6.0), p(7.0, -6.0)],
                    egui::Stroke::new(2.5, INK),
                ));
            } else {
                let s = egui::Stroke::new(2.5, FAIL_RED);
                ui.painter().line_segment([p(-6.0, -6.0), p(6.0, 6.0)], s);
                ui.painter().line_segment([p(-6.0, 6.0), p(6.0, -6.0)], s);
            }

            ui.allocate_ui_with_layout(
                egui::vec2(52.0, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(egui::RichText::new(c.ratio_text).size(16.0).color(MUTED));
                },
            );
            ui.label(egui::RichText::new(c.label).size(16.0).color(MUTED));
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

        let picking = !matches!(self.mode, Mode::Normal);
        let mut pick_target: Option<Target> = None;
        let mut swap_requested = false;

        let (bg_eff, fg_eff) = self.effective();
        let ratio = crate::color::contrast_ratio(fg_eff, bg_eff);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::WHITE)
                    .inner_margin(24.0),
            )
            .show(ui, |ui| {
                let col_w =
                    ((ui.available_width() - 3.0 * COL_GAP - SWAP_SIZE) / 3.0).max(0.0);

                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = COL_GAP;
                    let size = egui::vec2(col_w, ui.available_height());
                    let layout = egui::Layout::top_down(egui::Align::Min);

                    // Left: preview + criteria.
                    ui.allocate_ui_with_layout(size, layout, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        Self::draw_summary(ui, bg_eff, fg_eff, ratio);
                    });

                    // Middle: Foreground.
                    let row_y = ui
                        .allocate_ui_with_layout(size, layout, |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                            self.fg
                                .ui(ui, picking, || pick_target = Some(Target::Fg))
                        })
                        .inner;

                    let (slot, _) = ui.allocate_exact_size(
                        egui::vec2(SWAP_SIZE, 1.0),
                        egui::Sense::hover(),
                    );
                    let btn_rect = egui::Rect::from_center_size(
                        egui::pos2(slot.center().x, row_y),
                        egui::vec2(SWAP_SIZE, SWAP_SIZE),
                    );
                    if Self::swap_button(ui, btn_rect) {
                        swap_requested = true;
                    }

                    // Right: Background.
                    ui.allocate_ui_with_layout(size, layout, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        self.bg
                            .ui(ui, picking, || pick_target = Some(Target::Bg));
                    });
                });
            });

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

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::light();
    v.panel_fill = egui::Color32::WHITE;
    v.extreme_bg_color = egui::Color32::WHITE;
    let r = egui::CornerRadius::same(8);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0xCC));
    ctx.set_visuals(v);
}

pub fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 440.0])
            .with_min_inner_size([740.0, 420.0]),
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