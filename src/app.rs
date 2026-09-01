//! Main GUI: two color fields (foreground/background), a swap button, a
//! live preview, and WCAG pass/fail badges. Picking a color opens the
//! magnifier as a second egui viewport so the main window stays visible
//! while the loupe is up.
//!
//! Run with: `cargo run -- gui`

use eframe::egui;
use image::RgbImage;

use crate::color::Rgb;
use crate::magnifier::MagnifierState;
use crate::screencast::ScreenSource;

/// Stable id for the magnifier's viewport, so the same native window can be
/// referred to across frames (opened once, updated, then closed).
fn magnifier_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("magnifier")
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

    let hex = if let Some(stripped) = t.strip_prefix('#') {
        stripped
    } else {
        t
    };
    Rgb::from_hex(hex).ok()
}

/// One color row: label + swatch + hex field + pick button.
struct ColorRowState {
    label: &'static str,
    color: Rgb,
    /// Raw text in the hex field, kept separate from `color` so the user
    /// can type invalid/partial hex without it being clobbered every frame.
    hex_text: String,
    /// What to restore the field to if the user leaves it in an invalid
    /// state.
    last_valid_hex: String,
}

impl ColorRowState {
    fn new(label: &'static str, color: Rgb) -> Self {
        Self {
            label,
            color,
            hex_text: color.to_hex(),
            last_valid_hex: color.to_hex(),
        }
    }

    fn set_color(&mut self, color: Rgb) {
        self.color = color;
        self.hex_text = color.to_hex();
        self.last_valid_hex = color.to_hex();
    }

    /// Draws one row and returns true if `color` changed this frame (either
    /// from typing or from a pick), so the caller knows whether to
    /// recompute contrast.
    fn ui(&mut self, ui: &mut egui::Ui, picking: bool, on_pick: impl FnOnce()) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.add_sized([75.0, 20.0], egui::Label::new(self.label));

            // Swatch: border, rounded corners, filled with the current color.
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
            ui.painter().rect_filled(
                rect,
                4.0,
                egui::Color32::from_rgb(self.color.r, self.color.g, self.color.b),
            );
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x88)),
                egui::StrokeKind::Inside,
            );

            let field = ui.add(
                egui::TextEdit::singleline(&mut self.hex_text).desired_width(70.0),
            );

            if field.changed() {
                // Live-parse as the user types: update color+swatch if
                // valid, leave the text alone either way.
                if let Some(parsed) = parse_color(&self.hex_text) {
                    self.color = parsed;
                    self.last_valid_hex = parsed.to_hex();
                    changed = true;
                }
            }
            if field.lost_focus() {
                // Normalize on blur/Enter, or restore the last valid value
                // if what's left isn't parseable.
                self.hex_text = self.last_valid_hex.clone();
            }

            // Disabled mid-pick so a double click on either row can't start
            // two captures at once.
            let btn = ui.add_enabled(
                !picking,
                egui::Button::new("Pick").min_size(egui::vec2(48.0, 32.0)),
            );
            if btn.on_hover_text("Pick color from screen").clicked() {
                on_pick();
            }
        });

        changed
    }
}

struct App {
    mode: Mode,
    fg: ColorRowState,
    bg: ColorRowState,
    ratio: f64,
}

/// One WCAG criterion badge row.
struct Criterion {
    label: &'static str,
    threshold: f64,
}

impl Default for App {
    fn default() -> Self {
        let fg = ColorRowState::new("Foreground", Rgb::new(0x00, 0x00, 0x00));
        let bg = ColorRowState::new("Background", Rgb::new(0xFF, 0xFF, 0xFF));
        let ratio = crate::color::contrast_ratio(fg.color, bg.color);
        Self { mode: Mode::Normal, fg, bg, ratio }
    }
}

impl App {
    fn update_contrast(&mut self) {
        self.ratio = crate::color::contrast_ratio(self.fg.color, self.bg.color);
    }

    fn swap(&mut self) {
        let fg_color = self.fg.color;
        self.fg.set_color(self.bg.color);
        self.bg.set_color(fg_color);
        self.update_contrast();
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

    /// Draws the badge rows for one WCAG criterion group: a heading, then
    /// an indented Pass/Fail badge + description per row.
    fn draw_criteria(ui: &mut egui::Ui, ratio: f64, heading: &str, rows: &[Criterion]) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(heading).size(14.0).strong());
        });
        for row in rows {
            ui.horizontal(|ui| {
                let pass = ratio >= row.threshold;
                let (text, color) = if pass {
                    ("Pass", egui::Color32::from_rgb(0x2d, 0x9e, 0x2d))
                } else {
                    ("Fail", egui::Color32::from_rgb(0xFE, 0x41, 0x1A))
                };
                ui.add_sized(
                    [60.0, 16.0],
                    egui::Label::new(egui::RichText::new(text).size(12.0).strong().color(color)),
                );
                ui.label(egui::RichText::new(row.label).size(12.0));
            });
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Garantit qu'aucun fond noir opaque n'est appliqué par défaut avant le rendu
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe 0.36 hands us the root Ui directly rather than a
        // Context; ctx is still needed below for viewport commands and
        // show_viewport_immediate, so it's grabbed once up front.
        let ctx = ui.ctx().clone();
        let ctx = &ctx;

        // Main window: always drawn, every frame, regardless of mode. This
        // is what keeps it visible while the loupe viewport is up.
        let picking = !matches!(self.mode, Mode::Normal);
        let mut pick_target: Option<Target> = None;
        let mut contrast_dirty = false;

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("colza");
            ui.add_space(4.0);

            if self.fg.ui(ui, picking, || pick_target = Some(Target::Fg)) {
                contrast_dirty = true;
            }

            ui.horizontal(|ui| {
                if ui
                    .add_sized([60.0, 20.0], egui::Button::new("Swap"))
                    .clicked()
                {
                    self.swap();
                }
            });

            if self.bg.ui(ui, picking, || pick_target = Some(Target::Bg)) {
                contrast_dirty = true;
            }

            ui.separator();

            ui.horizontal(|ui| {
                // Preview swatch: fg-colored "Aa" on bg-colored background.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    4.0,
                    egui::Color32::from_rgb(self.bg.color.r, self.bg.color.g, self.bg.color.b),
                );
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x88)),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Aa",
                    egui::FontId::proportional(20.0),
                    egui::Color32::from_rgb(self.fg.color.r, self.fg.color.g, self.fg.color.b),
                );

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("{:.2}:1", self.ratio))
                        .size(20.0)
                        .strong(),
                );
            });

            ui.add_space(4.0);
            Self::draw_criteria(ui, self.ratio, "1.4.3 Contrast (Minimum) - AA", &[
                Criterion { label: "Regular text", threshold: 4.5 },
                Criterion { label: "Large text", threshold: 3.0 },
            ]);
            Self::draw_criteria(ui, self.ratio, "1.4.6 Contrast (Enhanced) - AAA", &[
                Criterion { label: "Regular text", threshold: 7.0 },
                Criterion { label: "Large text", threshold: 4.5 },
            ]);
            Self::draw_criteria(ui, self.ratio, "1.4.11 Non-text Contrast - AA", &[
                Criterion { label: "UI components", threshold: 3.0 },
            ]);
        });

        if contrast_dirty {
            self.update_contrast();
        }
        if let Some(target) = pick_target {
            self.start_picking(target);
        }

        // Mode-driven side effects: polling the capture, running the
        // magnifier viewport.
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

                        // Frame::NONE empêche de peindre l'arrière-plan opaque du thème egui
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
                    self.update_contrast();
                }
                if should_close {
                    self.mode = Mode::Normal;
                    ctx.send_viewport_cmd_to(
                        magnifier_viewport_id(),
                        egui::ViewportCommand::Close,
                    );
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint();
                }
            }
        }
    }
}

pub fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default().with_inner_size([320.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "colza",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
    .map_err(|err| anyhow::anyhow!("eframe failed: {err}"))?;

    Ok(())
}