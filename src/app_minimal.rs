//! Minimal proof-of-concept for embedding the magnifier inside a normal
//! eframe app, instead of spawning a separate `eframe::run_native` call
//! (which isn't possible from inside another app's `update()` — see the
//! doc comment on `MagnifierState` in `magnifier.rs`).
//!
//! Scope, deliberately small: ONE hex color field + a 🖍 button that opens
//! the magnifier, picks a color, and returns to the normal view. No fg/bg
//! pair, no contrast math, no WCAG badges yet — that's `widget.py`'s job
//! once this pattern is validated. See `ColorWidget` in `widget.py` for
//! what this grows into.
//!
//! Two fixes vs. the first version of this file:
//!
//! 1. **Screen capture no longer blocks `update()`.** Blocking inside
//!    `update()` blocks winit's event loop on the main thread; if the
//!    xdg-desktop-portal call needs anything from that same event loop to
//!    resolve (e.g. the compositor waiting on a window/focus event before
//!    answering DBus), you get a deadlock — which is the freeze-on-click
//!    that this was hitting. Capture now runs on a plain OS thread and
//!    reports back through an `mpsc` channel that `update()` polls
//!    non-blockingly every frame.
//!
//! 2. **The magnifier is a second, always-on-top viewport, not a takeover
//!    of the main window.** `ctx.show_viewport_immediate` lets one eframe
//!    `App` draw into more than one native window in the same `update()`
//!    call, so the small picker window stays visible and interactive while
//!    the fullscreen loupe is up, instead of being replaced by it.
//!
//! Run with: `cargo run -- gui`

use eframe::egui;
use image::RgbImage;

use crate::color::Rgb;
use crate::magnifier::MagnifierState;
use crate::portal;

/// Dedicated id for the magnifier's viewport, so we can refer to the same
/// native window across frames (open it once, keep updating it, close it
/// when done). `from_hash_of` isn't `const`, but it's deterministic for a
/// given input, so a small helper called at each use site is equivalent to
/// a constant without needing `once_cell`/`lazy_static`.
fn magnifier_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("magnifier")
}

/// What the app is doing right now, independent of which window(s) are on
/// screen. The main window is *always* shown; this only controls whether
/// the magnifier viewport is also shown and where its screenshot comes
/// from.
enum Mode {
    /// Just the main window: hex field + swatch + 🖍 button.
    Normal,
    /// 🖍 was clicked; a background thread is capturing the screen. The
    /// main window stays visible and responsive during this — capture
    /// happens off the UI thread specifically so it can never block
    /// `update()` (see module doc).
    Capturing(std::sync::mpsc::Receiver<anyhow::Result<RgbImage>>),
    /// Screenshot in hand; the magnifier viewport is open and sampling
    /// `state`. Boxed because `MagnifierState` owns a full-resolution
    /// `RgbImage` and we don't want that heavy to sit inline in `Mode`
    /// while we're in `Normal`/`Capturing`.
    Magnifying(Box<MagnifierState>),
}

struct MinimalApp {
    mode: Mode,

    /// The single color being edited.
    color: Rgb,
    /// Raw text in the hex field — kept separate from `color` so the user
    /// can type invalid/partial hex without it being clobbered every frame
    /// (same reasoning as `ColorRow._on_text_edited` vs `_last_valid_hex`
    /// in widget.py).
    hex_text: String,
}

impl Default for MinimalApp {
    fn default() -> Self {
        let color = Rgb::new(0x33, 0x66, 0x99);
        Self {
            mode: Mode::Normal,
            hex_text: color.to_hex(),
            color,
        }
    }
}

impl MinimalApp {
    /// Kicks off the pick flow: spawn a plain OS thread that builds its own
    /// short-lived tokio runtime just to run `portal::capture_screen()`
    /// (same one-shot-runtime trick `main.rs` uses for the `magnify`
    /// subcommand), and send the result back over a channel. Returns
    /// immediately — nothing here blocks the calling `update()` frame.
    fn start_picking(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(portal::capture_screen()));
            // Ignore send errors: if the receiver was dropped (e.g. the
            // app closed mid-capture), there's nothing to deliver to.
            let _ = tx.send(result);
        });

        self.mode = Mode::Capturing(rx);
    }

    fn set_color(&mut self, color: Rgb) {
        self.color = color;
        self.hex_text = color.to_hex();
    }

    /// Draws the small always-visible picker window's contents.
    fn draw_main_ui(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("colorpick (minimal)");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                // Swatch — direct equivalent of ColorRow.swatch in widget.py.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    4.0,
                    egui::Color32::from_rgb(self.color.r, self.color.g, self.color.b),
                );

                // Hex field — equivalent of ColorRow.field +
                // _on_text_edited/_on_editing_finished.
                let field =
                    ui.add(egui::TextEdit::singleline(&mut self.hex_text).desired_width(90.0));
                if field.changed() {
                    if let Ok(parsed) = Rgb::from_hex(&self.hex_text) {
                        self.color = parsed;
                    }
                    // Invalid/partial input: leave hex_text as-is (user is
                    // still typing), don't touch self.color.
                }
                if field.lost_focus() {
                    // Normalize on blur/Enter, same as
                    // _on_editing_finished: snap back to the last valid
                    // color's canonical hex string.
                    self.hex_text = self.color.to_hex();
                }

                // Pick button — equivalent of ColorRow.btn +
                // ColorWidget._open_picker. Disabled while a capture is
                // already in flight or the loupe is open, so a double
                // click can't start two captures at once.
                let picking = !matches!(self.mode, Mode::Normal);
                if ui
                    .add_enabled(!picking, egui::Button::new("🖍"))
                    .on_hover_text("Pick color")
                    .clicked()
                {
                    self.start_picking();
                }
                if picking {
                    ui.spinner();
                }
            });
        });
    }
}

impl eframe::App for MinimalApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The main window is drawn every frame regardless of mode, so it
        // stays visible and interactive the whole time — this is what
        // keeps the picker window on screen while the loupe is up.
        self.draw_main_ui(ctx);

        match &mut self.mode {
            Mode::Normal => {}

            Mode::Capturing(rx) => {
                match rx.try_recv() {
                    Ok(Ok(img)) => {
                        self.mode = Mode::Magnifying(Box::new(MagnifierState::new(img)));
                    }
                    Ok(Err(err)) => {
                        eprintln!("screen capture failed: {err}");
                        self.mode = Mode::Normal;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        // Still capturing; keep polling next frame without
                        // busy-looping the CPU.
                        ctx.request_repaint_after(std::time::Duration::from_millis(16));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        eprintln!("screen capture thread died without a result");
                        self.mode = Mode::Normal;
                    }
                }
            }

            Mode::Magnifying(state) => {
                let mut should_close = false;
                let mut picked_color = None;

                ctx.show_viewport_immediate(
                    magnifier_viewport_id(),
                    egui::ViewportBuilder::default()
                        .with_fullscreen(true)
                        .with_decorations(false)
                        .with_always_on_top()
                        .with_transparent(true),
                    |ctx, _class| {
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
                            // User closed the loupe window directly (e.g.
                            // Alt+F4 from the compositor) — treat like Esc.
                            should_close = true;
                        } else {
                            ctx.request_repaint();
                        }
                    },
                );

                if let Some(color) = picked_color {
                    self.set_color(color);
                }
                if should_close {
                    self.mode = Mode::Normal;
                    // Make sure the now-unused viewport is actually torn
                    // down rather than left as a stale closed window.
                    ctx.send_viewport_cmd_to(
                        magnifier_viewport_id(),
                        egui::ViewportCommand::Close,
                    );
                }
            }
        }
    }
}

pub fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([260.0, 120.0]),
        ..Default::default()
    };

    eframe::run_native(
        "colorpick (minimal)",
        options,
        Box::new(|_cc| Ok(Box::new(MinimalApp::default()))),
    )
    .map_err(|err| anyhow::anyhow!("eframe failed: {err}"))?;

    Ok(())
}