//! Fullscreen magnifier overlay — a Rust/egui port of the old PyQt6
//! `PickerOverlay` in `picker.py`.
//!
//! Same trick as `picker.py` and `shmooz`: the window covers the whole
//! screen and tracks the mouse *inside itself*, so we get "global" cursor
//! following without evdev or any special permissions. The only different
//! piece is where the screen image comes from (`screencast::Capture`
//! instead of shelling out to `spectacle`/`grim`) — and that it is a live
//! PipeWire feed rather than a still, so the sampled color tracks a playing
//! video under the loupe.
//!
//! Correspondence with `picker.py`:
//! - `PickerOverlay.__init__`            -> `MagnifierApp::new`
//! - `PickerOverlay.paintEvent`          -> `MagnifierApp::draw_magnifier`
//! - `PickerOverlay.mouseMoveEvent`      -> read from `ctx.input(|i| i.pointer...)`
//! - `PickerOverlay.mousePressEvent`     -> same, checking `pointer.primary_clicked()`
//! - `PickerOverlay.keyPressEvent` (Esc) -> `ctx.input(|i| i.key_pressed(egui::Key::Escape))`
//! - `launch_picker` / `_pick`           -> `MagnifierApp::run`, returns `Option<Rgb>`

use eframe::egui;
use image::RgbImage;

use crate::color::Rgb;

/// Pixels sampled per side of the magnifier grid (odd, so there is a
/// well-defined center cell under the cursor). Matches `LOUPE_PX` in
/// `picker.py`.
const LOUPE_PX: usize = 9;
/// On-screen size in points of the whole magnifier square. Matches
/// `LOUPE_SIZE` in `picker.py`.
const LOUPE_SIZE: f32 = 180.0;
const CELL: f32 = LOUPE_SIZE / LOUPE_PX as f32;
const HEX_BAND_HEIGHT: f32 = 28.0;
const LOUPE_OFFSET: f32 = 20.0;

/// Runs the fullscreen magnifier and blocks until the user picks a color
/// (left click / Enter) or cancels (Escape / closes the window).
///
/// `first_frame` is the image the loupe shows immediately, and `capture` is
/// the live session it keeps pulling newer frames from — so parking the
/// loupe on a playing video tracks the video rather than freezing on the
/// frame that happened to be on screen when the overlay opened. `capture`
/// is dropped when this returns, which stops the stream and closes the
/// portal session.
pub fn run(
    first_frame: RgbImage,
    capture: crate::screencast::Capture,
) -> anyhow::Result<Option<Rgb>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_fullscreen(true)
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true),
        ..Default::default()
    };

    // eframe owns the event loop and only returns once the window closes,
    // so we can't just return a value from inside the App; a channel is
    // the idiomatic way to hand a result back out. The sender is cloned
    // into the App and fired exactly once, right before the app requests
    // the window to close.
    let (tx, rx) = std::sync::mpsc::channel::<Rgb>();
    let app = MagnifierApp::new(first_frame, capture, tx);

    eframe::run_native(
        "colorpick magnifier",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    )
    .map_err(|err| anyhow::anyhow!("eframe failed: {err}"))?;

    // By the time run_native returns, the app has either sent exactly one
    // color (picked) or dropped the sender without sending (cancelled),
    // so a non-blocking try_recv is enough here.
    Ok(rx.try_recv().ok())
}

/// Everything the magnifier needs to sample colors from the captured
/// screenshot and map window-space points to image pixels. This is the
/// reusable core: both the standalone `MagnifierApp` (CLI `magnify`
/// subcommand, own eframe event loop) and an embedding app (e.g. a
/// "pick" mode inside a bigger eframe window) can hold one of these and
/// call `draw` / `handle_input` directly from their own `update()`.
///
/// This type deliberately knows nothing about PipeWire: to drive a live
/// view, the owner assigns a newer image to `screenshot` between frames
/// (both callers do this from `Capture::take_frame()`). Sampling reads
/// straight out of that `RgbImage` on the CPU — no GPU texture upload — so
/// replacing it per frame costs only the move.
pub struct MagnifierState {
    /// The image being sampled. Public so an owner driving a live feed can
    /// swap in a newer frame; a plain assignment is enough.
    pub screenshot: RgbImage,
    pub cursor_pos: egui::Pos2,
}

impl MagnifierState {
    pub fn new(screenshot: RgbImage) -> Self {
        Self {
            screenshot,
            cursor_pos: egui::Pos2::ZERO,
        }
    }

    pub fn color_at(&self, image_pos: (u32, u32)) -> Rgb {
        let x = image_pos.0.min(self.screenshot.width().saturating_sub(1));
        let y = image_pos.1.min(self.screenshot.height().saturating_sub(1));
        let px = self.screenshot.get_pixel(x, y);
        Rgb::new(px[0], px[1], px[2])
    }

    /// Maps a point in *window/logical* space to a pixel in the captured
    /// screenshot, accounting for the screenshot possibly being a
    /// different resolution than the logical window size (HiDPI), the
    /// same role `self.dpr` plays in `picker.py`.
    pub fn image_pos_for(&self, ctx: &egui::Context, logical: egui::Pos2) -> (u32, u32) {
        let screen_rect = ctx.screen_rect();
        let scale_x = self.screenshot.width() as f32 / screen_rect.width().max(1.0);
        let scale_y = self.screenshot.height() as f32 / screen_rect.height().max(1.0);
        (
            (logical.x * scale_x).round().max(0.0) as u32,
            (logical.y * scale_y).round().max(0.0) as u32,
        )
    }

    /// Reads pointer/keyboard input for this frame: updates `cursor_pos`
    /// (including arrow-key nudging, like `PickerOverlay.keyPressEvent`),
    /// and reports whether the user picked (click/Enter) or cancelled
    /// (Esc) this frame. Does not draw anything and does not close any
    /// window — the caller decides what "picked"/"cancelled" means.
    pub fn handle_input(&mut self, ctx: &egui::Context) -> MagnifierInput {
        let mut clicked_or_entered = false;
        let mut cancelled = false;

        // IMPORTANT: `ctx.input(...)` holds an internal lock on egui's
        // input state for the duration of the closure. Calling anything
        // that itself needs to lock `ctx` (like `ctx.screen_rect()`, which
        // `image_pos_for` calls) from *inside* this closure re-enters that
        // lock and deadlocks — this was the freeze-on-click bug. So this
        // closure only reads `input` and sets plain local flags; anything
        // that needs `ctx` again happens after the closure returns.
        ctx.input(|input| {
            // Only snap `cursor_pos` to the physical mouse position when
            // the mouse actually moved *this frame* (or on the very first
            // frame, when we haven't seen a pointer position yet). egui
            // redraws continuously while this viewport is open, and
            // `pointer.latest_pos()` returns the mouse's current position
            // on every single frame regardless of whether it moved — so
            // unconditionally assigning it here (as a previous version of
            // this function did) overwrote arrow-key nudges one frame
            // after they were applied, which looked like the loupe
            // flashing in place instead of moving. `pointer.delta()` is
            // egui's per-frame movement vector and is exactly zero when
            // the mouse hasn't moved, so it's the right signal to gate on.
            let mouse_moved = input.pointer.delta() != egui::Vec2::ZERO;
            if mouse_moved || self.cursor_pos == egui::Pos2::ZERO {
                if let Some(pos) = input.pointer.latest_pos() {
                    self.cursor_pos = pos;
                }
            }

            // Arrow keys nudge the cursor by one logical pixel, mirroring
            // `PickerOverlay.keyPressEvent`'s Left/Right/Up/Down handling.
            let mut delta = egui::Vec2::ZERO;
            if input.key_pressed(egui::Key::ArrowLeft) {
                delta.x -= 1.0;
            }
            if input.key_pressed(egui::Key::ArrowRight) {
                delta.x += 1.0;
            }
            if input.key_pressed(egui::Key::ArrowUp) {
                delta.y -= 1.0;
            }
            if input.key_pressed(egui::Key::ArrowDown) {
                delta.y += 1.0;
            }
            if delta != egui::Vec2::ZERO {
                self.cursor_pos += delta;
            }

            if input.pointer.primary_clicked() || input.key_pressed(egui::Key::Enter) {
                clicked_or_entered = true;
            }
            if input.key_pressed(egui::Key::Escape) {
                cancelled = true;
            }
        });

        let picked = if clicked_or_entered {
            let image_pos = self.image_pos_for(ctx, self.cursor_pos);
            Some(self.color_at(image_pos))
        } else {
            None
        };

        MagnifierInput { picked, cancelled }
    }

    /// Direct port of `PickerOverlay.paintEvent`.
    pub fn draw(&self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let painter = ui.painter();
        let screen_rect = ctx.screen_rect();

        let cursor = self.cursor_pos;
        let half = (LOUPE_PX / 2) as i64;

        // Flip the loupe to the opposite side of the cursor if it would
        // otherwise run off the screen — same logic as picker.py's
        // `lx`/`ly` adjustment.
        let mut loupe_origin = egui::pos2(cursor.x + LOUPE_OFFSET, cursor.y + LOUPE_OFFSET);
        if loupe_origin.x + LOUPE_SIZE > screen_rect.width() {
            loupe_origin.x = cursor.x - LOUPE_SIZE - LOUPE_OFFSET;
        }
        if loupe_origin.y + LOUPE_SIZE + HEX_BAND_HEIGHT + 2.0 > screen_rect.height() {
            loupe_origin.y = cursor.y - LOUPE_SIZE - LOUPE_OFFSET - HEX_BAND_HEIGHT - 2.0;
        }

        let center_image_pos = self.image_pos_for(ctx, cursor);

        for row in 0..LOUPE_PX {
            for col in 0..LOUPE_PX {
                let dx = col as i64 - half;
                let dy = row as i64 - half;
                let sample_x = (center_image_pos.0 as i64 + dx).max(0) as u32;
                let sample_y = (center_image_pos.1 as i64 + dy).max(0) as u32;
                let color = self.color_at((sample_x, sample_y));

                let cell_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        loupe_origin.x + col as f32 * CELL,
                        loupe_origin.y + row as f32 * CELL,
                    ),
                    egui::vec2(CELL, CELL),
                );
                painter.rect_filled(
                    cell_rect,
                    0.0,
                    egui::Color32::from_rgb(color.r, color.g, color.b),
                );
            }
        }

        // Highlight the exact center cell (the pixel that would be picked).
        let center_rect = egui::Rect::from_min_size(
            egui::pos2(
                loupe_origin.x + (LOUPE_PX / 2) as f32 * CELL,
                loupe_origin.y + (LOUPE_PX / 2) as f32 * CELL,
            ),
            egui::vec2(CELL, CELL),
        );
        painter.rect_stroke(
            center_rect.expand(1.0),
            0.0,
            egui::Stroke::new(1.0_f32, egui::Color32::BLACK),
        );
        painter.rect_stroke(
            center_rect,
            0.0,
            egui::Stroke::new(1.0_f32, egui::Color32::WHITE),
        );

        // Hex readout band under the loupe.
        let center_color = self.color_at(center_image_pos);
        let band_rect = egui::Rect::from_min_size(
            egui::pos2(loupe_origin.x, loupe_origin.y + LOUPE_SIZE + 2.0),
            egui::vec2(LOUPE_SIZE, HEX_BAND_HEIGHT),
        );
        painter.rect_filled(
            band_rect,
            0.0,
            egui::Color32::from_rgb(center_color.r, center_color.g, center_color.b),
        );

        let luminance = 0.299 * center_color.r as f32
            + 0.587 * center_color.g as f32
            + 0.114 * center_color.b as f32;
        let text_color = if luminance > 128.0 {
            egui::Color32::BLACK
        } else {
            egui::Color32::WHITE
        };
        painter.text(
            band_rect.center(),
            egui::Align2::CENTER_CENTER,
            center_color.to_hex(),
            egui::FontId::monospace(14.0),
            text_color,
        );
    }
}

/// Result of `MagnifierState::handle_input` for one frame: at most one of
/// these will be set (a click/Enter picks, Esc cancels; both can't happen
/// the same frame since Esc short-circuits nothing else here but callers
/// should just check `picked` first).
pub struct MagnifierInput {
    pub picked: Option<Rgb>,
    pub cancelled: bool,
}

/// Standalone eframe app wrapping a `MagnifierState`. Used by the CLI
/// `magnify` subcommand, which owns its own window/event loop via
/// `run()`. An app that wants to embed the magnifier as a *mode* inside
/// a bigger window (e.g. a "pick" button in a larger UI) should hold a
/// `MagnifierState` directly instead and call `handle_input`/`draw` from
/// its own `update()` — see `MagnifierState`'s doc comment.
struct MagnifierApp {
    state: MagnifierState,
    /// Held for the lifetime of the overlay so the feed stays live; see
    /// `run`'s doc comment.
    capture: crate::screencast::Capture,
    result_tx: std::sync::mpsc::Sender<Rgb>,
}

impl MagnifierApp {
    fn new(
        first_frame: RgbImage,
        capture: crate::screencast::Capture,
        result_tx: std::sync::mpsc::Sender<Rgb>,
    ) -> Self {
        Self {
            state: MagnifierState::new(first_frame),
            capture,
            result_tx,
        }
    }
}

impl eframe::App for MagnifierApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Swap in the newest frame before reading input, so a click samples
        // the image the user is actually looking at. `None` means nothing
        // new since the last repaint — keep showing what we have.
        if let Some(frame) = self.capture.take_frame() {
            self.state.screenshot = frame;
        }

        let input = self.state.handle_input(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                self.state.draw(ctx, ui);
            });

        if let Some(color) = input.picked {
            // Ignore send errors: if the receiver was already dropped
            // (caller gave up), there's nothing useful to do here.
            let _ = self.result_tx.send(color);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if input.cancelled {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            // Repaint continuously so the magnifier follows the mouse
            // smoothly even without new input events arriving (egui
            // defaults to reactive/on-demand painting).
            ctx.request_repaint();
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Fully transparent clear so `with_transparent(true)` actually
        // shows through anywhere we don't paint, matching
        // WA_TranslucentBackground in picker.py.
        [0.0, 0.0, 0.0, 0.0]
    }
}