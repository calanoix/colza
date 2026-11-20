//! Fullscreen magnifier overlay.
//!
//! The window covers the whole screen and tracks the mouse inside itself,
//! which gives "global" cursor following without evdev or any special
//! permissions. The screen image comes from `screencast::ScreenSource`:
//! normally a live PipeWire feed, so the sampled color tracks a playing
//! video under the loupe, and a single still screenshot when the user
//! declined screen sharing.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

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
/// Crosshair geometry, in points: the empty square left around the
/// picked pixel, and how far each arm reaches from the center.
const CROSSHAIR_GAP: f32 = 4.0;
const CROSSHAIR_ARM: f32 = 12.0;
/// How long an area this overlay painted stays masked out of incoming
/// frames (see [`MagnifierState::accept_frame`]). It must outlast the
/// capture pipeline's latency — compositor, PipeWire, our thread, the
/// next repaint — which is typically 2–4 display frames. Generous on
/// purpose: erring long only means a spot the loupe just left shows
/// slightly older screen content for a moment; erring short lets the
/// overlay leak back into the loupe, which is the bug this prevents.
const OVERLAY_MASK_TIME: Duration = Duration::from_millis(300);

/// Everything the magnifier needs to sample colors from the captured
/// screenshot and map window-space points to image pixels. This is the
/// reusable core: both the standalone `MagnifierApp` (CLI `magnify`
/// subcommand, own eframe event loop) and the embedding `app::App` (a
/// "pick" mode inside the main window) hold one of these and call
/// `draw`/`handle_input` directly from their own `ui()`.
///
/// This type knows nothing about PipeWire: to drive a live view, the owner
/// hands each newer image to [`Self::accept_frame`] between frames (from
/// `Capture::take_frame()`). Sampling reads straight out of that
/// `RgbImage` on the CPU.
pub struct MagnifierState {
    /// The image being sampled. Replace it through [`Self::accept_frame`]
    /// rather than by assignment, so our own overlay gets masked out.
    pub screenshot: RgbImage,
    /// Window-space areas this overlay painted recently, newest last, for
    /// [`Self::accept_frame`] to mask out of incoming frames.
    overlay_history: VecDeque<(Instant, egui::Rect)>,
    /// Image pixels per window point, as of the last `draw`. Kept because
    /// `accept_frame` runs outside the magnifier's viewport, where
    /// `ctx.input()` describes the main window instead.
    scale: Option<egui::Vec2>,
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
            overlay_history: VecDeque::new(),
            scale: None,
        }
    }

    /// Takes a newer captured frame, with our own overlay removed.
    ///
    /// The ScreenCast stream films the whole screen, and this overlay is a
    /// window on it: the loupe and the crosshair end up in the very frames
    /// they display. `CursorMode::Hidden` only excludes the *system*
    /// cursor, and no portal lets a client exclude its own window. Because
    /// a frame arrives a few display frames after it was captured, what
    /// leaks in is the overlay at an *earlier* position — a stray
    /// crosshair or loupe flickering inside the loupe as it moves.
    ///
    /// We do know exactly where we painted, though. Every area the
    /// overlay covered within the last [`OVERLAY_MASK_TIME`] keeps its
    /// pixels from the previous image (itself already cleaned this way)
    /// instead of taking the new frame's; the rest of the screen updates
    /// normally. The first frame is captured before the overlay is shown,
    /// so the chain starts clean.
    ///
    /// The cost is that a masked area doesn't update while the overlay
    /// keeps covering it. That is why the crosshair is masked arm by arm
    /// rather than as one square: the pixel being picked sits in its gap
    /// and must stay live (a video under the loupe keeps playing).
    pub fn accept_frame(&mut self, mut frame: RgbImage) {
        let now = Instant::now();
        while self
            .overlay_history
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > OVERLAY_MASK_TIME)
        {
            self.overlay_history.pop_front();
        }

        // A resolution change (e.g. the monitor was reconfigured) leaves
        // nothing to restore from; take the frame as is.
        let scale = self
            .scale
            .filter(|_| frame.dimensions() == self.screenshot.dimensions());
        if let Some(scale) = scale {
            let (w, h) = frame.dimensions();
            let row_bytes = w as usize * 3;
            let prev = self.screenshot.as_raw();
            let next: &mut [u8] = &mut frame;
            for (_, r) in &self.overlay_history {
                let x0 = ((r.min.x * scale.x).floor().max(0.0) as u32).min(w) as usize;
                let x1 = ((r.max.x * scale.x).ceil().max(0.0) as u32).min(w) as usize;
                let y0 = ((r.min.y * scale.y).floor().max(0.0) as u32).min(h) as usize;
                let y1 = ((r.max.y * scale.y).ceil().max(0.0) as u32).min(h) as usize;
                for y in y0..y1 {
                    let span = y * row_bytes + x0 * 3..y * row_bytes + x1 * 3;
                    next[span.clone()].copy_from_slice(&prev[span]);
                }
            }
        }

        self.screenshot = frame;
    }

    /// Records an area painted this frame, for [`Self::accept_frame`].
    /// While the overlay holds still, it paints the same areas every
    /// frame; refreshing the existing entry's time keeps the history (and
    /// the copying above) from growing with the frame rate.
    fn record_overlay(&mut self, rect: egui::Rect) {
        let now = Instant::now();
        if let Some(entry) = self.overlay_history.iter_mut().rev().find(|(_, r)| *r == rect) {
            entry.0 = now;
            return;
        }
        self.overlay_history.push_back((now, rect));
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

    /// Inverse of [`Self::image_pos_for`]: the window-space center of an
    /// image pixel. Drawing the crosshair here rather than at the raw
    /// `cursor_pos` frames exactly the pixel that will be picked, which
    /// matters on HiDPI, where one logical point spans several pixels.
    fn logical_center_of(&self, ctx: &egui::Context, image_pos: (u32, u32)) -> egui::Pos2 {
        let screen_rect = ctx.input(|i| i.viewport_rect());
        let scale_x = self.screenshot.width() as f32 / screen_rect.width().max(1.0);
        let scale_y = self.screenshot.height() as f32 / screen_rect.height().max(1.0);
        egui::pos2(
            (image_pos.0 as f32 + 0.5) / scale_x,
            (image_pos.1 as f32 + 0.5) / scale_y,
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

    pub fn draw(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let painter = ui.painter();
        let screen_rect = ctx.input(|i| i.viewport_rect());
        self.scale = Some(egui::vec2(
            self.screenshot.width() as f32 / screen_rect.width().max(1.0),
            self.screenshot.height() as f32 / screen_rect.height().max(1.0),
        ));

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

        // The whole loupe stack (grid, hex band, caption), plus a margin
        // for the center highlight's outer stroke and antialiasing.
        self.record_overlay(
            egui::Rect::from_min_size(
                loupe_origin,
                egui::vec2(LOUPE_SIZE, LOUPE_SIZE + below_grid),
            )
            .expand(2.0),
        );

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

        self.draw_crosshair(ctx, painter, center_image_pos);

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

impl MagnifierState {
    /// Our own crosshair, standing in for the system cursor (which the
    /// owner hides while the magnifier is up — see `app.rs`). It follows
    /// `cursor_pos`, so it moves with arrow-key nudges as well as the
    /// mouse.
    ///
    /// Four arms around an empty center, so the pixel being picked stays
    /// visible, each drawn white over a wider black stroke so it reads on
    /// any background.
    fn draw_crosshair(&mut self, ctx: &egui::Context, painter: &egui::Painter, image_pos: (u32, u32)) {
        const OUTLINE: f32 = 3.0;
        let c = self.logical_center_of(ctx, image_pos);
        let arms = [
            egui::vec2(1.0, 0.0),
            egui::vec2(-1.0, 0.0),
            egui::vec2(0.0, 1.0),
            egui::vec2(0.0, -1.0),
        ];
        for dir in arms {
            // Each arm's footprint, padded 1pt for antialiasing. Stops
            // short of the center: the gap (and the picked pixel in it)
            // must stay live — see `accept_frame`.
            let (a, b) = (c + dir * CROSSHAIR_GAP, c + dir * CROSSHAIR_ARM);
            self.record_overlay(egui::Rect::from_two_pos(a, b).expand(OUTLINE / 2.0 + 1.0));
        }
        for (width, color) in [(OUTLINE, egui::Color32::BLACK), (1.0, egui::Color32::WHITE)] {
            for dir in arms {
                painter.line_segment(
                    [c + dir * CROSSHAIR_GAP, c + dir * CROSSHAIR_ARM],
                    egui::Stroke::new(width, color),
                );
            }
        }
    }
}

/// Result of `MagnifierState::handle_input` for one frame: at most one of
/// these is set (a click/Enter picks, Esc cancels).
pub struct MagnifierInput {
    pub picked: Option<Rgb>,
    pub cancelled: bool,
}
#[cfg(test)]
mod tests {
    use super::*;

    fn solid(v: u8) -> RgbImage {
        RgbImage::from_pixel(100, 50, image::Rgb([v, v, v]))
    }

    /// Where the overlay painted, the new frame's pixels (which would show
    /// the overlay itself) are replaced by the previous image's.
    #[test]
    fn accept_frame_masks_recent_overlay() {
        let mut state = MagnifierState::new(solid(10));
        state.scale = Some(egui::vec2(2.0, 2.0)); // HiDPI: 2 px per point
        state.record_overlay(egui::Rect::from_min_max(egui::pos2(5.0, 5.0), egui::pos2(10.0, 10.0)));

        state.accept_frame(solid(200));
        let at = |s: &MagnifierState, x, y| s.screenshot.get_pixel(x, y)[0];
        assert_eq!(at(&state, 10, 10), 10, "inside the mask (points 5..10 = px 10..20)");
        assert_eq!(at(&state, 19, 19), 10);
        assert_eq!(at(&state, 20, 20), 200, "just outside");
        assert_eq!(at(&state, 0, 0), 200);

        // Once the overlay has been gone long enough, the area updates.
        state.overlay_history[0].0 -= OVERLAY_MASK_TIME * 2;
        state.accept_frame(solid(250));
        assert_eq!(at(&state, 10, 10), 250);
    }

    #[test]
    fn holding_still_does_not_grow_the_history() {
        let mut state = MagnifierState::new(solid(0));
        let r = egui::Rect::from_min_size(egui::pos2(1.0, 1.0), egui::vec2(3.0, 3.0));
        for _ in 0..100 {
            state.record_overlay(r);
        }
        assert_eq!(state.overlay_history.len(), 1);
    }

    #[test]
    fn resolution_change_takes_the_frame_as_is() {
        let mut state = MagnifierState::new(solid(10));
        state.scale = Some(egui::vec2(1.0, 1.0));
        state.record_overlay(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)));
        state.accept_frame(RgbImage::from_pixel(60, 40, image::Rgb([99, 99, 99])));
        assert_eq!(state.screenshot.get_pixel(5, 5)[0], 99);
    }
}