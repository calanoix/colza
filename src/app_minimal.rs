//! 1:1 Rust/egui port of `widget.py`'s `ColorWidget` (+ `ColorRow`), with
//! `picker.py`'s magnifier embedded as a second viewport (see the module
//! doc from the previous minimal version for why: a native `run_native`
//! call can't be nested inside another app's `update()`, and a *second*
//! viewport — rather than a full takeover — is what keeps the main window
//! visible while the loupe is up).
//!
//! Layout/behavior correspondence with widget.py:
//! - `ColorRow`                          -> `ColorRowState` + `ColorRowState::ui`
//! - `ColorRow._parse_color`             -> `parse_color`
//! - `ColorRow._on_text_edited`          -> live-parse in `ColorRowState::ui`, on `changed()`
//! - `ColorRow._on_editing_finished`     -> normalize/restore on `lost_focus()`
//! - `ColorRow._on_mouse_press` (select-all on click) -> NOT ported, see note
//!   in `ColorRowState::ui` next to the hex `TextEdit`
//! - `ColorWidget.__init__` layout       -> `eframe::App::update`'s `CentralPanel` block
//! - `ColorWidget._open_picker`          -> `MinimalApp::start_picking`
//! - `ColorWidget._swap`                 -> `MinimalApp::swap`
//! - `ColorWidget._update_contrast`      -> `MinimalApp::update_contrast` + `draw_criteria`
//! - `PickerOverlay` cross cursor        -> `ctx.set_cursor_icon(CursorIcon::Crosshair)` while magnifying
//! - `PickerOverlay` arrow-key movement  -> `MagnifierState::handle_input` (magnifier.rs, unchanged)
//!
//! Run with: `cargo run -- gui`

use eframe::egui;
use image::RgbImage;

use crate::color::Rgb;
use crate::magnifier::MagnifierState;
use crate::screencast::Capture;

/// Dedicated id for the magnifier's viewport, so we can refer to the same
/// native window across frames (open it once, keep updating it, close it
/// when done). `from_hash_of` isn't `const`, but it's deterministic for a
/// given input, so a small helper called at each use site is equivalent to
/// a constant without needing `once_cell`/`lazy_static`.
fn magnifier_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("magnifier")
}

/// Which color field a `Mode::Capturing`/`Mode::Magnifying` pick is
/// targeting. Equivalent to `ColorWidget._open_picker`'s `target: ColorRow`
/// parameter — we can't store a `&mut ColorRowState` across frames the way
/// Python stores a bound closure, so we store which one instead and look
/// it up again once the color comes back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Fg,
    Bg,
}

/// What the app is doing right now, independent of which window(s) are on
/// screen. The main window is *always* shown; this only controls whether
/// the magnifier viewport is also shown and where its screenshot comes
/// from.
enum Mode {
    /// Just the main window.
    Normal,
    /// 🖍 was clicked for `Target`; a background task is opening a capture
    /// session. The main window stays visible and responsive during this —
    /// capture happens off the UI thread specifically so it can never
    /// block `update()` (a previous version of this file deadlocked here).
    Capturing(
        Target,
        std::sync::mpsc::Receiver<anyhow::Result<(Capture, RgbImage)>>,
    ),
    /// First frame in hand; the magnifier viewport is open and sampling
    /// `state` for `Target`.
    ///
    /// The `Capture` is held here for the whole time the loupe is up, which
    /// is what makes it a *live* view: `update()` pulls the newest frame
    /// from it each repaint. Dropping this variant (pick or cancel) stops
    /// the stream and closes the portal session.
    ///
    /// `MagnifierState` is boxed because it owns a full-resolution
    /// `RgbImage` and we don't want that heavy to sit inline in `Mode`
    /// while we're in `Normal`/`Capturing`.
    Magnifying(Target, Box<MagnifierState>, Capture),
}

/// Parses `#rrggbb`, `rrggbb`, or `rgb(r, g, b)` — direct port of
/// `ColorRow._parse_color`.
fn parse_color(text: &str) -> Option<Rgb> {
    let t = text.trim();

    if t.to_lowercase().starts_with("rgb") {
        // Minimal `rgb(r, g, b)` scanner — no regex crate in this project,
        // so we parse the three integers by hand rather than pull one in
        // for a single call site.
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

/// Equivalent of `ColorRow`: label + swatch + hex field + pick button, plus
/// the text-editing state Python keeps on the widget instance.
struct ColorRowState {
    label: &'static str,
    color: Rgb,
    /// Raw text in the hex field — kept separate from `color` so the user
    /// can type invalid/partial hex without it being clobbered every frame
    /// (`_on_text_edited` updates `self.color` live but leaves the field
    /// text alone; `_on_editing_finished` is what snaps the *text* back).
    hex_text: String,
    /// Mirrors `_last_valid_hex`: what to restore the field to if the user
    /// leaves it in an invalid state.
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
    /// from typing or from the swatch — used by the caller to know whether
    /// to recompute contrast, mirroring `_notify_parent`).
    fn ui(&mut self, ui: &mut egui::Ui, picking: bool, on_pick: impl FnOnce()) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.add_sized([75.0, 20.0], egui::Label::new(self.label));

            // Swatch — border #888, radius 4px, 28x28, matching
            // ColorRow._update_swatch's stylesheet exactly. Two calls
            // (fill + stroke) instead of the 5-arg `Painter::rect(...)`,
            // since that signature's `StrokeKind` parameter was added in a
            // fairly recent egui and isn't safe to assume without a
            // Cargo.lock to check against.
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
            ui.painter().rect_filled(
                rect,
                4.0,
                egui::Color32::from_rgb(self.color.r, self.color.g, self.color.b),
            );
            ui.painter()
                .rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x88)));

            // Hex field, min width 60 (we use a fixed width close to it,
            // egui doesn't have a separate min-width knob for TextEdit).
            //
            // NB: ColorRow._on_mouse_press in widget.py also selects all
            // text on click, so retyping doesn't require clearing the
            // field first. egui's `TextEditState` cursor-manipulation API
            // (`CCursorRange`/`set_char_range`) has moved between modules
            // across egui versions and isn't worth pinning down without a
            // Cargo.lock to check against — this is the one behavior
            // knowingly dropped from the 1:1 port. If you want it back:
            // on `field.gained_focus() || field.clicked()`, load the
            // field's `TextEditState` via `TextEdit::load_state`, set its
            // cursor range to span the whole string, and store it back —
            // check `egui::text_edit::TextEditState` docs for your pinned
            // egui version for the exact path.
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.hex_text).desired_width(70.0),
            );

            if field.changed() {
                // ColorRow._on_text_edited: live-parse, update color+swatch
                // if valid, leave the text alone either way (user is still
                // typing).
                if let Some(parsed) = parse_color(&self.hex_text) {
                    self.color = parsed;
                    self.last_valid_hex = parsed.to_hex();
                    changed = true;
                }
            }
            if field.lost_focus() {
                // ColorRow._on_editing_finished: normalize on blur/Enter,
                // or restore the last valid value if what's left isn't
                // parseable.
                self.hex_text = self.last_valid_hex.clone();
            }

            // Pick button, 32x32, tooltip "Pick color" — matches
            // ColorRow.btn. Disabled mid-pick so a double click on either
            // row can't start two captures at once.
            //
            // NB: widget.py used the 🖍 emoji here. egui's built-in font
            // only ships a small, curated set of emoji glyphs (see
            // `egui::special_emojis` for the full list) — 🖍 isn't one of
            // them, so it rendered as a missing-glyph box (□) in the
            // screenshot this was tested against. "Pick" reads clearly
            // without depending on emoji coverage; swap back to an emoji
            // only if you add a font that actually contains it (e.g. via
            // `egui_extras::install_image_loaders` doesn't cover this —
            // you'd need something like a Noto Color Emoji font loaded
            // through `egui::FontDefinitions`).
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

/// Equivalent of `ColorWidget`.
struct MinimalApp {
    mode: Mode,
    fg: ColorRowState,
    bg: ColorRowState,
    /// Cached badge states, recomputed by `update_contrast` whenever fg/bg
    /// changes — mirrors `self.criteria_labels` + `_update_contrast`'s
    /// per-frame recompute in Python (there it's driven by signals; here
    /// we just recompute eagerly since it's cheap).
    ratio: f64,
}

/// One WCAG criterion badge row — matches the "Pass/Fail badge +
/// description" rows built in `ColorWidget.__init__` (regular/large text
/// under 1.4.3 and 1.4.6, UI components under 1.4.11).
struct Criterion {
    label: &'static str,
    threshold: f64,
}

impl Default for MinimalApp {
    fn default() -> Self {
        let fg = ColorRowState::new("Foreground", Rgb::new(0x00, 0x00, 0x00));
        let bg = ColorRowState::new("Background", Rgb::new(0xFF, 0xFF, 0xFF));
        let ratio = crate::color::contrast_ratio(fg.color, bg.color);
        Self { mode: Mode::Normal, fg, bg, ratio }
    }
}

impl MinimalApp {
    fn update_contrast(&mut self) {
        self.ratio = crate::color::contrast_ratio(self.fg.color, self.bg.color);
    }

    fn swap(&mut self) {
        let fg_color = self.fg.color;
        self.fg.set_color(self.bg.color);
        self.bg.set_color(fg_color);
        self.update_contrast();
    }

    /// Kicks off the pick flow for `target`: spawn `Capture::open()` as a
    /// task on the process-wide runtime and send the resulting live session
    /// (plus its first frame) back over a channel. Returns immediately —
    /// nothing here blocks the calling `update()` frame (blocking here
    /// previously caused a freeze).
    ///
    /// A `tokio::spawn` on `runtime::shared()`, rather than the OS thread +
    /// per-click `current_thread` runtime this used to do: that pattern
    /// panicked on the *second* pick once ashpd moved to its tokio backend,
    /// because ashpd's cached D-Bus connection outlived the runtime it was
    /// bound to (see runtime.rs). With one long-lived multi-threaded
    /// runtime, spawning is both correct and cheaper than a thread.
    ///
    /// Thread topology for a "Pick" click: UI thread spawns a task ->
    /// runtime worker drives the portal D-Bus round-trip -> a dedicated
    /// `colza-pipewire` thread runs the blocking PipeWire mainloop for as
    /// long as the loupe is open -> the session handle comes back to the UI
    /// thread via the `rx` channel below, and frames after the first arrive
    /// through the handle's own mutex slot. Every hop is a channel, a mutex
    /// or a task boundary, not a re-entrant lock, so this doesn't
    /// reintroduce the deadlock class
    /// the app already fixed once.
    fn start_picking(&mut self, target: Target) {
        let (tx, rx) = std::sync::mpsc::channel();

        match crate::runtime::shared() {
            Ok(rt) => {
                rt.spawn(async move {
                    let _ = tx.send(Capture::open().await);
                });
            }
            // Sending the error rather than dropping `tx` silently: the
            // `Capturing` arm of `update()` surfaces a received `Err` to
            // the user, whereas a hung-up channel would just look like a
            // pick that never finished.
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

    /// Draws the badge rows for one WCAG criterion group, matching the
    /// "heading, then indented Pass/Fail badge + description" layout built
    /// by hand in `ColorWidget.__init__`.
    fn draw_criteria(ui: &mut egui::Ui, ratio: f64, heading: &str, rows: &[Criterion]) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(heading).size(14.0).strong());
        });
        for row in rows {
            ui.horizontal(|ui| {
                let pass = ratio >= row.threshold;
                // ASCII rather than ✓/✗: those render as missing-glyph
                // boxes with egui's default font, same issue as the 🖍/⇅
                // buttons above.
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

impl eframe::App for MinimalApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Main window: always drawn, every frame, regardless of mode ──
        // (this is what keeps it visible while the loupe viewport is up).
        let picking = !matches!(self.mode, Mode::Normal);
        let mut pick_target: Option<Target> = None;
        let mut contrast_dirty = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Linux Contrast Checker");
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
                // Preview swatch: fg-colored "Aa" on bg-colored background,
                // 48x48, matches ColorWidget.preview's stylesheet.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    4.0,
                    egui::Color32::from_rgb(self.bg.color.r, self.bg.color.g, self.bg.color.b),
                );
                ui.painter()
                    .rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(0x88)));
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

        // ── Mode-driven side effects: polling the capture, running the
        // magnifier viewport ──
        match &mut self.mode {
            Mode::Normal => {}

            Mode::Capturing(target, rx) => match rx.try_recv() {
                Ok(Ok((capture, first_frame))) => {
                    self.mode = Mode::Magnifying(
                        *target,
                        Box::new(MagnifierState::new(first_frame)),
                        capture,
                    );
                    // Without this, nothing schedules the *next* frame —
                    // the one that will actually call show_viewport_immediate
                    // for the magnifier below — until some external input
                    // event (e.g. mouse movement) happens to trigger a
                    // redraw. That's why the loupe used to only appear
                    // once you moved the mouse: the mode flip above is
                    // silent otherwise. request_repaint() asks winit to
                    // schedule another update() right away regardless of
                    // input.
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

                // The live feed, in one line: swap in the newest frame the
                // capture thread has produced, if any. `None` means nothing
                // new arrived since the last repaint, so the existing image
                // stays — the loupe simply keeps showing the last thing the
                // screen looked like. The assignment is a move, not a copy,
                // so resolution doesn't matter here.
                //
                // Done before `handle_input` so that a click in this frame
                // samples the frame the user is actually looking at.
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
                    |ctx, _class| {
                        // picker.py's PickerOverlay sets a cross cursor for
                        // the whole time the overlay is up
                        // (self.setCursor(Qt.CursorShape.CrossCursor) in
                        // __init__). egui's equivalent is setting the
                        // cursor icon each frame this viewport is drawn.
                        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);

                        let input = state.handle_input(ctx);

                        egui::CentralPanel::default()
                            .frame(egui::Frame::none())
                            .show(ctx, |ui| {
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
                    // Same reasoning as the Capturing -> Magnifying
                    // transition above: force the next frame to be
                    // scheduled rather than relying on it happening to
                    // coincide with another input event.
                    ctx.request_repaint();
                } else {
                    // Requested on the *parent* context, not just the
                    // viewport's own inside the closure above. A viewport
                    // opened with `show_viewport_immediate` is only painted
                    // as part of the parent's pass, so without this the
                    // loupe redraws only when some input event happens to
                    // wake the parent — which is enough to track the mouse
                    // (mouse motion *is* an event) but not to show a live
                    // feed of a video playing under a stationary cursor.
                    ctx.request_repaint();
                }
            }
        }
    }
}

pub fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([320.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Linux Contrast Checker",
        options,
        Box::new(|_cc| Ok(Box::new(MinimalApp::default()))),
    )
    .map_err(|err| anyhow::anyhow!("eframe failed: {err}"))?;

    Ok(())
}