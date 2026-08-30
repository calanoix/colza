//! Fullscreen magnifier overlay — a Rust/egui port of the old PyQt6
//! `PickerOverlay` in `picker.py`.
//!
//! Same trick as `picker.py` and `shmooz`: the window covers the whole
//! screen and tracks the mouse *inside itself*, so we get "global" cursor
//! following without evdev or any special permissions. The only different
//! piece is where the screen image comes from (`portal::capture_screen`
//! instead of shelling out to `spectacle`/`grim`).
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
/// `screenshot` is the already-captured, already-decoded full-screen image
/// (see `portal::capture_screen`); capturing happens *before* opening the
/// window so the picked color always corresponds to what the screen looked
/// like right before the overlay appeared, not a stale later frame.
pub fn run(screenshot: RgbImage) -> anyhow::Result<Option<Rgb>> {
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
    let app = MagnifierApp::new(screenshot, tx);

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

struct MagnifierApp {
    screenshot: RgbImage,
    cursor_pos: egui::Pos2,
    result_tx: std::sync::mpsc::Sender<Rgb>,
}

impl MagnifierApp {
    fn new(screenshot: RgbImage, result_tx: std::sync::mpsc::Sender<Rgb>) -> Self {
        Self {
            screenshot,
            cursor_pos: egui::Pos2::ZERO,
            result_tx,
        }
    }

    fn color_at(&self, image_pos: (u32, u32)) -> Rgb {
        let x = image_pos.0.min(self.screenshot.width().saturating_sub(1));
        let y = image_pos.1.min(self.screenshot.height().saturating_sub(1));
        let px = self.screenshot.get_pixel(x, y);
        Rgb::new(px[0], px[1], px[2])
    }

    /// Maps a point in *window/logical* space to a pixel in the captured
    /// screenshot, accounting for the screenshot possibly being a
    /// different resolution than the logical window size (HiDPI), the
    /// same role `self.dpr` plays in `picker.py`.
    fn image_pos_for(&self, ctx: &egui::Context, logical: egui::Pos2) -> (u32, u32) {
        let screen_rect = ctx.screen_rect();
        let scale_x = self.screenshot.width() as f32 / screen_rect.width().max(1.0);
        let scale_y = self.screenshot.height() as f32 / screen_rect.height().max(1.0);
        (
            (logical.x * scale_x).round().max(0.0) as u32,
            (logical.y * scale_y).round().max(0.0) as u32,
        )
    }

    /// Direct port of `PickerOverlay.paintEvent`.
    fn draw_magnifier(&self, ctx: &egui::Context, ui: &mut egui::Ui) {
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

impl eframe::App for MagnifierApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ---- input: mouse (equivalent of mouseMoveEvent/mousePressEvent) ----
        let mut picked: Option<Rgb> = None;
        let mut cancelled = false;

        ctx.input(|input| {
            if let Some(pos) = input.pointer.latest_pos() {
                self.cursor_pos = pos;
            }
            if input.pointer.primary_clicked() || input.key_pressed(egui::Key::Enter) {
                let image_pos = self.image_pos_for(ctx, self.cursor_pos);
                picked = Some(self.color_at(image_pos));
            }
            if input.key_pressed(egui::Key::Escape) {
                cancelled = true;
            }
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                self.draw_magnifier(ctx, ui);
            });

        if let Some(color) = picked {
            // Ignore send errors: if the receiver was already dropped
            // (caller gave up), there's nothing useful to do here.
            let _ = self.result_tx.send(color);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if cancelled {
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