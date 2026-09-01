//! Fullscreen magnifier overlay.
//!
//! The window covers the whole screen and tracks the mouse inside itself,
//! which gives "global" cursor following without evdev or any special
//! permissions. The screen image comes from `screencast::ScreenSource`:
//! normally a live PipeWire feed, so the sampled color tracks a playing
//! video under the loupe, and a single still screenshot when the user
//! declined screen sharing.

use eframe::egui;
use image::RgbImage;

use crate::color::Rgb;

/// Pixels sampled per side of the magnifier grid (odd, so there is a
/// well-defined center cell under the cursor).
const LOUPE_PX: usize = 9;
/// On-screen size in points of the whole magnifier square.
const LOUPE_SIZE: f32 = 180.0;
const CELL: f32 = LOUPE_SIZE / LOUPE_PX as f32;
const HEX_BAND_HEIGHT: f32 = 28.0;
/// Height of the "still image" caption drawn under the hex band when the
/// source isn't live.
const STILL_CAPTION_HEIGHT: f32 = 14.0;
const LOUPE_OFFSET: f32 = 20.0;

/// Everything the magnifier needs to sample colors from the captured
/// screenshot and map window-space points to image pixels. This is the
/// reusable core: both the standalone `MagnifierApp` (CLI `magnify`
/// subcommand, own eframe event loop) and the embedding `app::App` (a
/// "pick" mode inside the main window) hold one of these and call
/// `draw`/`handle_input` directly from their own `ui()`.
///
/// This type knows nothing about PipeWire: to drive a live view, the owner
/// assigns a newer image to `screenshot` between frames (both callers do
/// this from `Capture::take_frame()`). Sampling reads straight out of that
/// `RgbImage` on the CPU, so replacing it per frame costs only the move.
pub struct MagnifierState {
    /// The image being sampled. Public so an owner driving a live feed can
    /// swap in a newer frame with a plain assignment.
    pub screenshot: RgbImage,
    pub cursor_pos: egui::Pos2,
    /// Whether `screenshot` is being refreshed. `false` draws a small
    /// caption under the loupe, since the degraded path differs in two
    /// ways that would otherwise look like a bug: the image never updates,
    /// and the mouse cursor is baked into it.
    pub live: bool,
}

impl MagnifierState {
    pub fn new(screenshot: RgbImage) -> Self {
        Self {
            screenshot,
            cursor_pos: egui::Pos2::ZERO,
            live: true,
        }
    }

    pub fn color_at(&self, image_pos: (u32, u32)) -> Rgb {
        let x = image_pos.0.min(self.screenshot.width().saturating_sub(1));
        let y = image_pos.1.min(self.screenshot.height().saturating_sub(1));
        let px = self.screenshot.get_pixel(x, y);
        Rgb::new(px[0], px[1], px[2])
    }

    /// Maps a point in window/logical space to a pixel in the captured
    /// screenshot, accounting for the screenshot possibly being a
    /// different resolution than the logical window size (HiDPI).
    pub fn image_pos_for(&self, ctx: &egui::Context, logical: egui::Pos2) -> (u32, u32) {
        // `Context::screen_rect()` was removed in egui 0.36; the
        // viewport's rect now lives on `InputState`. Safe to call
        // `ctx.input()` here: this function is never invoked from inside
        // another `ctx.input(...)` closure (see the note on
        // `handle_input` below for why that matters).
        let screen_rect = ctx.input(|i| i.viewport_rect());
        let scale_x = self.screenshot.width() as f32 / screen_rect.width().max(1.0);
        let scale_y = self.screenshot.height() as f32 / screen_rect.height().max(1.0);
        (
            (logical.x * scale_x).round().max(0.0) as u32,
            (logical.y * scale_y).round().max(0.0) as u32,
        )
    }

    /// Reads pointer/keyboard input for this frame: updates `cursor_pos`
    /// (including arrow-key nudging), and reports whether the user picked
    /// (click/Enter) or cancelled (Esc) this frame. Does not draw anything
    /// and does not close any window — the caller decides what
    /// picked/cancelled means.
    pub fn handle_input(&mut self, ctx: &egui::Context) -> MagnifierInput {
        let mut clicked_or_entered = false;
        let mut cancelled = false;

        // `ctx.input(...)` holds an internal lock on egui's input state for
        // the duration of the closure. Calling anything that itself needs
        // to lock `ctx` (like `ctx.input(|i| i.viewport_rect())`, which
        // `image_pos_for` calls) from inside this closure would re-enter
        // that lock and deadlock. So this closure only reads `input` and
        // sets plain local flags; anything needing `ctx` again happens
        // after it returns.
        ctx.input(|input| {
            // Only snap `cursor_pos` to the physical mouse position when
            // the mouse actually moved this frame (or on the very first
            // frame). egui redraws continuously while this viewport is
            // open, and `pointer.latest_pos()` returns the mouse's current
            // position every frame regardless of movement, so
            // unconditionally assigning it would overwrite arrow-key
            // nudges the frame after they're applied. `pointer.delta()` is
            // exactly zero when the mouse hasn't moved, so it's the right
            // signal to gate on.
            let mouse_moved = input.pointer.delta() != egui::Vec2::ZERO;
            if mouse_moved || self.cursor_pos == egui::Pos2::ZERO {
                if let Some(pos) = input.pointer.latest_pos() {
                    self.cursor_pos = pos;
                }
            }

            // Arrow keys nudge the cursor by one logical pixel.
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

    pub fn draw(&self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let painter = ui.painter();
        let screen_rect = ctx.input(|i| i.viewport_rect());

        let cursor = self.cursor_pos;
        let half = (LOUPE_PX / 2) as i64;

        // Flip the loupe to the opposite side of the cursor if it would
        // otherwise run off the screen.
        let mut loupe_origin = egui::pos2(cursor.x + LOUPE_OFFSET, cursor.y + LOUPE_OFFSET);
        if loupe_origin.x + LOUPE_SIZE > screen_rect.width() {
            loupe_origin.x = cursor.x - LOUPE_SIZE - LOUPE_OFFSET;
        }
        // Everything drawn below the grid, which the flip above must
        // account for or the bottom-most element lands off-screen near the
        // lower edge. The still-mode caption is part of that stack when
        // present.
        let below_grid = HEX_BAND_HEIGHT
            + 2.0
            + if self.live {
                0.0
            } else {
                STILL_CAPTION_HEIGHT + 2.0
            };
        if loupe_origin.y + LOUPE_SIZE + below_grid > screen_rect.height() {
            loupe_origin.y = cursor.y - LOUPE_SIZE - LOUPE_OFFSET - below_grid;
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
            egui::StrokeKind::Inside,
        );
        painter.rect_stroke(
            center_rect,
            0.0,
            egui::Stroke::new(1.0_f32, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
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

        if !self.live {
            // Drawn with its own dark backing rather than straight onto
            // the screen: this sits on top of arbitrary desktop content,
            // and plain text over an unknown background is a coin flip for
            // legibility.
            let caption = "still image · cursor shown";
            let caption_rect = egui::Rect::from_min_size(
                egui::pos2(loupe_origin.x, band_rect.max.y + 2.0),
                egui::vec2(LOUPE_SIZE, STILL_CAPTION_HEIGHT),
            );
            painter.rect_filled(
                caption_rect,
                0.0,
                egui::Color32::from_black_alpha(0xCC),
            );
            painter.text(
                caption_rect.center(),
                egui::Align2::CENTER_CENTER,
                caption,
                egui::FontId::proportional(10.0),
                egui::Color32::from_gray(0xDD),
            );
        }
    }
}

/// Result of `MagnifierState::handle_input` for one frame: at most one of
/// these is set (a click/Enter picks, Esc cancels).
pub struct MagnifierInput {
    pub picked: Option<Rgb>,
    pub cancelled: bool,
}