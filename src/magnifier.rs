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
const LOUPE_GRID: usize = 9;
/// On-screen size of the whole magnifier square, in points.
const LOUPE_SIZE: f32 = 180.0;
/// On-screen size of one sampled pixel, in points.
const CELL_SIZE: f32 = LOUPE_SIZE / LOUPE_GRID as f32;
/// Height of the hex readout band under the grid, in points.
const HEX_BAND_HEIGHT: f32 = 28.0;
/// Height of the "still image" caption drawn under the hex band when the
/// source isn't live.
const STILL_CAPTION_HEIGHT: f32 = 14.0;
/// Distance from the cursor to the loupe's nearest corner, in points.
const LOUPE_OFFSET: f32 = 20.0;
/// Length of each crosshair arm, in points. Where the arms *start* isn't
/// a constant: see [`crosshair_gap`].
const CROSSHAIR_ARM_LENGTH: f32 = 8.0;
/// Width of the crosshair's black outline (the white core is 1pt).
const CROSSHAIR_OUTLINE_WIDTH: f32 = 3.0;
/// Margin around anything the overlay paints, for antialiasing.
const AA_MARGIN: f32 = 1.0;
/// How long an area this overlay painted stays masked out of incoming
/// frames (see [`MagnifierState::accept_frame`]). It must outlast the
/// capture pipeline's latency (compositor, PipeWire, the capture thread,
/// the next repaint), which is typically 2–4 display frames. Generous on
/// purpose: erring long only means a spot the loupe just left shows
/// slightly older screen content for a moment; erring short lets the
/// overlay leak back into the loupe, which is the bug this prevents.
const OVERLAY_MASK_TIME: Duration = Duration::from_millis(300);

/// Everything the magnifier needs to sample colors from the captured
/// image and map window-space points to image pixels. `app::App` holds
/// one while picking, and calls `handle_input`/`draw` from the magnifier
/// viewport's own `ui()`.
///
/// This type knows nothing about PipeWire: to drive a live view, the owner
/// hands each newer image to [`Self::accept_frame`] between frames (from
/// `ScreenSource::take_frame()`). Sampling reads straight out of that
/// `RgbImage` on the CPU.
pub struct MagnifierState {
    /// The image being sampled. Replace it through [`Self::accept_frame`]
    /// rather than by assignment, so the overlay itself gets masked out.
    pub image: RgbImage,
    /// Window-space areas this overlay painted recently, newest last, for
    /// [`Self::accept_frame`] to mask out of incoming frames.
    overlay_history: VecDeque<(Instant, egui::Rect)>,
    /// Image pixels per window point, as of the last `draw`. Kept because
    /// `accept_frame` runs outside the magnifier's viewport, where
    /// `ctx.input()` describes the main window instead.
    scale: Option<egui::Vec2>,
    pub cursor_pos: egui::Pos2,
    /// Whether `image` is being refreshed. `false` draws a small caption
    /// under the loupe, since a frozen image would otherwise look like a
    /// bug.
    pub live: bool,
    /// Whether the mouse cursor is baked into `image` (a screenshot), which
    /// the caption then says too.
    pub cursor_in_image: bool,
    /// Paint `image` itself as the overlay's background, instead of leaving
    /// it transparent over the real screen. See [`needs_opaque_backdrop`].
    pub opaque_backdrop: bool,
    /// `image` uploaded for [`Self::opaque_backdrop`], made on first use.
    backdrop_texture: Option<egui::TextureHandle>,
}

/// Whether the magnifier must paint the captured image as its own
/// background, because a transparent fullscreen window wouldn't show the
/// screen under it on this desktop.
///
/// Transparency works only where the compositor keeps drawing the other
/// windows under a fullscreen one, and that turned out to be the
/// exception. Seen so far:
///
/// - KWin (KDE): works, the magnifier stays transparent and live.
/// - Mutter (GNOME, and the desktops built on it): paints black behind
///   every fullscreen Wayland window that no opaque surface covers, on
///   purpose and with no way out for clients (since 43.1,
///   `maybe_configure_black_background` in `meta-window-actor-wayland.c`).
/// - Sway (1.9, tested): hides the workspace's other windows behind a
///   fullscreen one and shows black instead, as tiling compositors do.
///   Hyprland and COSMIC's tiling are expected to behave alike.
///
/// So everywhere but KDE, the overlay paints the captured image itself,
/// which looks the same, and colza captures with a screenshot only (see
/// [`live_capture_supported`]). Unknown desktops get this too: a frozen
/// image beats a black screen with wrong colors.
///
/// Decided by `XDG_CURRENT_DESKTOP` containing "KDE".
/// `COLZA_OPAQUE_MAGNIFIER=1` or `=0` forces it either way, to test a
/// desktop not on the list yet.
pub fn needs_opaque_backdrop() -> bool {
    if let Some(forced) = std::env::var_os("COLZA_OPAQUE_MAGNIFIER") {
        return forced != "0";
    }
    let is_kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktops| {
        desktops.split(':').any(|d| d.eq_ignore_ascii_case("KDE"))
    });
    !is_kde
}

/// Whether screen sharing is offered at all: only where the magnifier can
/// stay a transparent pane over the live screen (KDE so far, see
/// [`needs_opaque_backdrop`]).
///
/// Elsewhere it would bring nothing over a screenshot. With the overlay
/// opaque, a share films colza's own painting instead of the screen, so
/// only its first frame is usable: a still image, after an extra
/// permission dialog. And it didn't even keep the cursor out reliably:
/// in VM tests, Sway's and COSMIC's first frame had it baked in anyway.
/// Those desktops get the Screenshot portal only.
pub fn live_capture_supported() -> bool {
    !needs_opaque_backdrop()
}

impl MagnifierState {
    pub fn new(image: RgbImage) -> Self {
        Self {
            image,
            cursor_pos: egui::Pos2::ZERO,
            live: true,
            cursor_in_image: false,
            opaque_backdrop: false,
            backdrop_texture: None,
            overlay_history: VecDeque::new(),
            scale: None,
        }
    }

    /// Takes a newer captured frame, with the overlay itself removed.
    ///
    /// The ScreenCast stream films the whole screen, and this overlay is a
    /// window on it: the loupe and the crosshair end up in the very frames
    /// they display. `CursorMode::Hidden` only excludes the *system*
    /// cursor, and no portal lets a client exclude its own window. Because
    /// a frame arrives a few display frames after it was captured, what
    /// leaks in is the overlay at an *earlier* position: a stray crosshair
    /// or loupe flickering inside the loupe as it moves.
    ///
    /// The overlay does know exactly where it painted, though. Every area
    /// it covered within the last [`OVERLAY_MASK_TIME`] keeps its pixels
    /// from the previous image (itself already cleaned this way) instead
    /// of taking the new frame's; the rest of the screen updates normally.
    /// The first frame is captured before the overlay is shown, so the
    /// chain starts clean.
    ///
    /// The cost is that a masked area doesn't update while the overlay
    /// keeps covering it. That is why the crosshair is masked arm by arm
    /// rather than as one square, and why its arms start outside the
    /// sampled grid ([`crosshair_gap`]): every pixel the loupe shows stays
    /// live (a video under it keeps playing).
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
            .filter(|_| frame.dimensions() == self.image.dimensions());
        if let Some(scale) = scale {
            let (w, h) = frame.dimensions();
            let row_bytes = w as usize * 3;
            let previous = self.image.as_raw();
            let next: &mut [u8] = &mut frame;
            for (_, rect) in &self.overlay_history {
                let x0 = ((rect.min.x * scale.x).floor().max(0.0) as u32).min(w) as usize;
                let x1 = ((rect.max.x * scale.x).ceil().max(0.0) as u32).min(w) as usize;
                let y0 = ((rect.min.y * scale.y).floor().max(0.0) as u32).min(h) as usize;
                let y1 = ((rect.max.y * scale.y).ceil().max(0.0) as u32).min(h) as usize;
                for y in y0..y1 {
                    let span = y * row_bytes + x0 * 3..y * row_bytes + x1 * 3;
                    next[span.clone()].copy_from_slice(&previous[span]);
                }
            }
        }

        self.image = frame;
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
        let x = image_pos.0.min(self.image.width().saturating_sub(1));
        let y = image_pos.1.min(self.image.height().saturating_sub(1));
        let px = self.image.get_pixel(x, y);
        Rgb::new(px[0], px[1], px[2])
    }

    /// Maps a point in window/logical space to a pixel in the captured
    /// image, accounting for the image possibly being a different
    /// resolution than the logical window size (HiDPI).
    pub fn image_pos_for(&self, ctx: &egui::Context, logical: egui::Pos2) -> (u32, u32) {
        pixel_at(logical, self.current_scale(ctx))
    }

    /// Image pixels per window point, per axis, for the viewport `ctx` is
    /// currently drawing.
    ///
    /// `Context::screen_rect()` was removed in egui 0.36; the viewport's
    /// rect now lives on `InputState`. Must not be called from inside
    /// another `ctx.input(...)` closure (see the note in `handle_input`).
    fn current_scale(&self, ctx: &egui::Context) -> egui::Vec2 {
        let screen_rect = ctx.input(|i| i.viewport_rect());
        image_scale(self.image.dimensions(), screen_rect.size())
    }

    /// Inverse of [`Self::image_pos_for`]: the window-space center of an
    /// image pixel. Drawing the crosshair here rather than at the raw
    /// `cursor_pos` frames exactly the pixel that will be picked, which
    /// matters on HiDPI, where one logical point spans several pixels.
    fn logical_center_of(&self, ctx: &egui::Context, image_pos: (u32, u32)) -> egui::Pos2 {
        pixel_center(image_pos, self.current_scale(ctx))
    }

    /// Reads pointer/keyboard input for this frame: updates `cursor_pos`
    /// (including arrow-key nudging), and reports whether the user picked
    /// (click/Enter) or cancelled (Escape) this frame. Draws nothing and
    /// closes no window: the caller decides what picked/cancelled means.
    pub fn handle_input(&mut self, ctx: &egui::Context) -> MagnifierInput {
        let mut clicked_or_entered = false;
        let mut cancelled = false;
        let mut nudge: (i64, i64) = (0, 0);

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

            // Arrow keys: collected here, applied after the closure,
            // since that needs the scale and so `ctx` again.
            if input.key_pressed(egui::Key::ArrowLeft) {
                nudge.0 -= 1;
            }
            if input.key_pressed(egui::Key::ArrowRight) {
                nudge.0 += 1;
            }
            if input.key_pressed(egui::Key::ArrowUp) {
                nudge.1 -= 1;
            }
            if input.key_pressed(egui::Key::ArrowDown) {
                nudge.1 += 1;
            }

            if input.pointer.primary_clicked() || input.key_pressed(egui::Key::Enter) {
                clicked_or_entered = true;
            }
            if input.key_pressed(egui::Key::Escape) {
                cancelled = true;
            }
        });

        // Arrow keys move the picked pixel by exactly one *image* pixel,
        // then put the cursor at that pixel's center. Stepping one window
        // point instead (as this used to) moves 1.25 pixels at 125%
        // display scaling, so after rounding sometimes 1 pixel and
        // sometimes 2, and 2 pixels at 200%, skipping every other one.
        if nudge != (0, 0) {
            let scale = self.current_scale(ctx);
            let (x, y) = pixel_at(self.cursor_pos, scale);
            let (w, h) = self.image.dimensions();
            let step = |v: u32, d: i64, len: u32| (v as i64 + d).clamp(0, len.max(1) as i64 - 1) as u32;
            self.cursor_pos = pixel_center((step(x, nudge.0, w), step(y, nudge.1, h)), scale);
        }

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
        self.scale = Some(image_scale(self.image.dimensions(), screen_rect.size()));

        if self.opaque_backdrop {
            let image = &self.image;
            let texture = self.backdrop_texture.get_or_insert_with(|| {
                let size = [image.width() as usize, image.height() as usize];
                // Nearest: the image is the screen at its own resolution,
                // drawn 1:1, and must not be smoothed.
                ctx.load_texture(
                    "magnifier-backdrop",
                    egui::ColorImage::from_rgb(size, image.as_raw()),
                    egui::TextureOptions::NEAREST,
                )
            });
            painter.image(
                texture.id(),
                screen_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        let cursor = self.cursor_pos;
        let half = (LOUPE_GRID / 2) as i64;

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

        for row in 0..LOUPE_GRID {
            for col in 0..LOUPE_GRID {
                let dx = col as i64 - half;
                let dy = row as i64 - half;
                let sample_x = (center_image_pos.0 as i64 + dx).max(0) as u32;
                let sample_y = (center_image_pos.1 as i64 + dy).max(0) as u32;
                let color = self.color_at((sample_x, sample_y));

                let cell_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        loupe_origin.x + col as f32 * CELL_SIZE,
                        loupe_origin.y + row as f32 * CELL_SIZE,
                    ),
                    egui::vec2(CELL_SIZE, CELL_SIZE),
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
                loupe_origin.x + (LOUPE_GRID / 2) as f32 * CELL_SIZE,
                loupe_origin.y + (LOUPE_GRID / 2) as f32 * CELL_SIZE,
            ),
            egui::vec2(CELL_SIZE, CELL_SIZE),
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
            let caption = if self.cursor_in_image {
                "still image · cursor shown"
            } else {
                "still image"
            };
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

    /// The magnifier's own crosshair, standing in for the system cursor
    /// (which the owner hides while the magnifier is up, see `app.rs`). It
    /// follows `cursor_pos`, so it moves with arrow-key nudges as well as
    /// the mouse.
    ///
    /// Four arms around an empty center, so the pixel being picked stays
    /// visible, each drawn white over a wider black stroke so it reads on
    /// any background.
    fn draw_crosshair(&mut self, ctx: &egui::Context, painter: &egui::Painter, image_pos: (u32, u32)) {
        let center = self.logical_center_of(ctx, image_pos);
        let gap = crosshair_gap(self.scale.unwrap_or(egui::Vec2::splat(1.0)));
        for (a, b) in crosshair_arms(center, gap) {
            self.record_overlay(crosshair_arm_footprint(a, b));
        }
        for (width, color) in [
            (CROSSHAIR_OUTLINE_WIDTH, egui::Color32::BLACK),
            (1.0, egui::Color32::WHITE),
        ] {
            for (a, b) in crosshair_arms(center, gap) {
                painter.line_segment([a, b], egui::Stroke::new(width, color));
            }
        }
    }
}

/// Distance from the crosshair's center to where its arms start, in
/// points: just far enough that nothing the crosshair paints (outline and
/// antialiasing included) lands on a pixel the loupe samples.
///
/// That's what keeps the whole 9×9 grid live. The ScreenCast stream films
/// the overlay itself, so pixels under the crosshair can't be shown as
/// they really are; [`MagnifierState::accept_frame`] substitutes the last
/// clean value for them, which freezes them. With the arms inside the
/// grid, those frozen cells stood still in the middle of a playing video.
/// Starting the arms outside it means there is nothing to freeze while
/// the crosshair holds still. (Moving, it still masks the spots it just
/// left for a moment, which the motion hides.)
///
/// Depends on the HiDPI scale, since the grid's extent is fixed in image
/// pixels: the outermost sampled pixel reaches `LOUPE_GRID / 2 + 0.5`
/// pixels from the center one's center. The smaller of the two axis
/// scales gives the larger, safe distance.
fn crosshair_gap(scale: egui::Vec2) -> f32 {
    let scale = scale.x.min(scale.y).max(f32::EPSILON);
    let grid_reach = ((LOUPE_GRID / 2) as f32 + 0.5) / scale;
    // Half a pixel more than strictly needed, so a stroke landing exactly
    // on a pixel boundary can't bleed into the outermost cell.
    grid_reach + 0.5 / scale + CROSSHAIR_OUTLINE_WIDTH / 2.0 + AA_MARGIN
}

/// The four arms as segments, outward from the gap around `center`.
fn crosshair_arms(center: egui::Pos2, gap: f32) -> [(egui::Pos2, egui::Pos2); 4] {
    [
        egui::vec2(1.0, 0.0),
        egui::vec2(-1.0, 0.0),
        egui::vec2(0.0, 1.0),
        egui::vec2(0.0, -1.0),
    ]
    .map(|dir| (center + dir * gap, center + dir * (gap + CROSSHAIR_ARM_LENGTH)))
}

/// Everything an arm from `a` to `b` can paint: the segment widened by
/// the outline and antialiasing. (Butt caps: nothing goes past `a`/`b`
/// along the arm but the antialiasing.)
fn crosshair_arm_footprint(a: egui::Pos2, b: egui::Pos2) -> egui::Rect {
    egui::Rect::from_two_pos(a, b).expand(CROSSHAIR_OUTLINE_WIDTH / 2.0 + AA_MARGIN)
}

/// Image pixels per window point, per axis: how much bigger the captured
/// image is than the window (1.25 at 125% display scaling, and so on).
fn image_scale((w, h): (u32, u32), viewport: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        w as f32 / viewport.x.max(1.0),
        h as f32 / viewport.y.max(1.0),
    )
}

/// The image pixel under window point `logical`.
///
/// Truncated, not rounded: pixel `i` covers `[i, i + 1)` in image space,
/// so a point anywhere inside it maps to `i`. Rounding (as this once did)
/// picked the next pixel whenever the pointer sat in the right or bottom
/// half of one, and disagreed with [`pixel_center`], so the crosshair and
/// the picked pixel could be half a pixel apart.
fn pixel_at(logical: egui::Pos2, scale: egui::Vec2) -> (u32, u32) {
    (
        (logical.x * scale.x).floor().max(0.0) as u32,
        (logical.y * scale.y).floor().max(0.0) as u32,
    )
}

/// The window point at the center of an image pixel; the inverse of
/// [`pixel_at`].
fn pixel_center((x, y): (u32, u32), scale: egui::Vec2) -> egui::Pos2 {
    egui::pos2((x as f32 + 0.5) / scale.x, (y as f32 + 0.5) / scale.y)
}

/// Result of `MagnifierState::handle_input` for one frame: at most one of
/// these is set (a click/Enter picks, Escape cancels).
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
        let at = |s: &MagnifierState, x, y| s.image.get_pixel(x, y)[0];
        assert_eq!(at(&state, 10, 10), 10, "inside the mask (points 5..10 = px 10..20)");
        assert_eq!(at(&state, 19, 19), 10);
        assert_eq!(at(&state, 20, 20), 200, "just outside");
        assert_eq!(at(&state, 0, 0), 200);

        // Once the overlay has been gone long enough, the area updates.
        state.overlay_history[0].0 -= OVERLAY_MASK_TIME * 2;
        state.accept_frame(solid(250));
        assert_eq!(at(&state, 10, 10), 250);
    }

    /// The crosshair never paints onto a pixel the loupe samples, at any
    /// HiDPI scale, so none of the grid gets frozen by the mask.
    #[test]
    fn crosshair_stays_off_the_sampled_grid() {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let center = egui::pos2(500.5 / scale, 300.5 / scale); // a pixel center
            let half = (LOUPE_GRID / 2) as f32;
            // The sampled grid, in points: pixel edges, not centers.
            let grid = egui::Rect::from_center_size(
                center,
                egui::Vec2::splat((2.0 * half + 1.0) / scale),
            );
            let gap = crosshair_gap(egui::Vec2::splat(scale));
            for (a, b) in crosshair_arms(center, gap) {
                let footprint = crosshair_arm_footprint(a, b);
                let overlap = footprint.intersect(grid);
                assert!(
                    overlap.width() <= 0.0 || overlap.height() <= 0.0,
                    "scale {scale}: arm {footprint:?} overlaps grid {grid:?}"
                );
            }
        }
    }

    /// At any display scale, `pixel_at` of a pixel's center is that
    /// pixel, so one arrow press (pixel +1, then its center) moves exactly
    /// one pixel. The bug was 1 or 2 at 125%.
    #[test]
    fn pixel_center_roundtrips_at_fractional_scales() {
        for s in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 3.0] {
            let scale = egui::Vec2::splat(s);
            for x in 0..4000u32 {
                assert_eq!(pixel_at(pixel_center((x, x), scale), scale), (x, x), "scale {s}");
            }
        }
    }

    #[test]
    fn pixel_at_truncates() {
        let scale = egui::Vec2::splat(1.25);
        // Point 10 → image 12.5: inside pixel 12, not "rounded" to 13.
        assert_eq!(pixel_at(egui::pos2(10.0, 10.0), scale), (12, 12));
        assert_eq!(pixel_at(egui::pos2(-3.0, 0.0), scale), (0, 0));
    }

    #[test]
    fn holding_still_does_not_grow_the_history() {
        let mut state = MagnifierState::new(solid(0));
        let rect = egui::Rect::from_min_size(egui::pos2(1.0, 1.0), egui::vec2(3.0, 3.0));
        for _ in 0..100 {
            state.record_overlay(rect);
        }
        assert_eq!(state.overlay_history.len(), 1);
    }

    #[test]
    fn resolution_change_takes_the_frame_as_is() {
        let mut state = MagnifierState::new(solid(10));
        state.scale = Some(egui::vec2(1.0, 1.0));
        state.record_overlay(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)));
        state.accept_frame(RgbImage::from_pixel(60, 40, image::Rgb([99, 99, 99])));
        assert_eq!(state.image.get_pixel(5, 5)[0], 99);
    }
}