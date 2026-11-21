//! Follows the desktop's text size, so colza's text grows when the user
//! enlarges it system-wide (WCAG 1.4.4 asks that text can be resized; for
//! someone who already did so in their desktop settings, an app that
//! ignores it effectively hasn't been).
//!
//! The result is a *factor* applied as egui's zoom (see `app.rs`), which
//! scales the whole layout, not just the glyphs: colza's layout is fixed
//! in points, so scaling text alone would make it overflow.
//!
//! KDE only for now. Plasma has no text-scaling factor as such, only a
//! font size; the factor is that size relative to Plasma's default (10 pt),
//! the size colza's layout was designed against. It's read through the
//! Settings portal, which `xdg-desktop-portal-kde` backs with kdeglobals:
//! the `font` key of the `org.kde.kdeglobals.General` group, a QFont
//! string such as `Noto Sans,11,-1,5,400,0,0,0,0,0,0,0,0,0,0,1`.
//! (Checked against the portal's `settings.cpp`: it exposes every
//! kdeglobals group under that prefix, and re-emits `font` whenever the
//! Fonts settings page applies a change.)
//!
//! Every failure means factor 1.0: no portal, another desktop, or a
//! Plasma whose font was never changed from the default (the key is then
//! absent from kdeglobals, and the portal reports it as missing). All
//! three have the same right answer, so they aren't told apart.

use std::sync::mpsc::Sender;
use std::time::Duration;

use ashpd::desktop::settings::Settings;
use ashpd::zvariant::Value;
use eframe::egui;
use futures_util::StreamExt;

const NAMESPACE: &str = "org.kde.kdeglobals.General";
const KEY: &str = "font";

/// Plasma's default general font size, in points.
const KDE_DEFAULT_PT: f32 = 10.0;

/// How long startup waits for the portal before going with 1.0. The read
/// is a local D-Bus round-trip (milliseconds); the cap is for a portal
/// that is slow to start or hung, which must not hold the window back.
const STARTUP_TIMEOUT: Duration = Duration::from_millis(500);

/// The factor for a QFont string, or `None` if it can't be read.
///
/// QFont's format is `family,pointSize,pixelSize,…`. Point size is a
/// float (`10.5` is legal), and -1 when the font is sized in pixels
/// instead, in which case the pixel size is converted at Qt's 96 dpi
/// reference (1 pt = 4/3 px).
///
/// Clamped to 0.75–3: below, text gets too small to be a sensible
/// request; above, the window wouldn't fit any screen.
pub fn factor_from_qfont(font: &str) -> Option<f32> {
    let mut fields = font.split(',').skip(1);
    let pt: f32 = fields.next()?.trim().parse().ok()?;
    let pt = if pt > 0.0 {
        pt
    } else {
        let px: f32 = fields.next()?.trim().parse().ok()?;
        if px <= 0.0 {
            return None;
        }
        px * 0.75
    };
    Some((pt / KDE_DEFAULT_PT).clamp(0.75, 3.0))
}

/// The current factor, read once at startup, before the window exists,
/// so the window opens at the right size instead of visibly growing.
/// Blocks for at most [`STARTUP_TIMEOUT`]; 1.0 on any failure.
///
/// Must be called outside any async context (see `runtime::shared`).
pub fn read_blocking() -> f32 {
    let Ok(rt) = crate::runtime::shared() else {
        return 1.0;
    };
    rt.block_on(async {
        tokio::time::timeout(STARTUP_TIMEOUT, read())
            .await
            .ok()
            .flatten()
            .unwrap_or(1.0)
    })
}

async fn read() -> Option<f32> {
    let settings = Settings::new().await.ok()?;
    let font: String = settings.read(NAMESPACE, KEY).await.ok()?;
    factor_from_qfont(&font)
}

/// Sends a new factor whenever the user changes the font in Plasma's
/// settings, then asks `ctx` for a repaint so the UI picks it up at once
/// rather than at the next input event.
///
/// Runs for the life of the process on the shared runtime. If the portal
/// is unavailable it simply never sends; startup already fell back to 1.0.
pub fn watch(ctx: egui::Context, tx: Sender<f32>) {
    let Ok(rt) = crate::runtime::shared() else {
        return;
    };
    rt.spawn(async move {
        let Ok(settings) = Settings::new().await else {
            return;
        };
        // The unfiltered stream, filtered here, rather than ashpd's
        // `receive_setting_changed_with_args::<String>`: that one converts
        // the value directly, and fails if the backend wraps the string in
        // one more variant layer, which `read()` tolerates and is a
        // legitimate encoding of a `v` argument.
        let Ok(mut changes) = settings.receive_setting_changed().await else {
            return;
        };
        while let Some(setting) = changes.next().await {
            if setting.namespace() != NAMESPACE || setting.key() != KEY {
                continue;
            }
            let font = match &**setting.value() {
                Value::Str(s) => s.as_str().to_owned(),
                Value::Value(inner) => match &**inner {
                    Value::Str(s) => s.as_str().to_owned(),
                    _ => continue,
                },
                _ => continue,
            };
            // A font reset to the default comes back as the default size,
            // so `None` here is only a malformed string: keep the current
            // factor rather than guess.
            if let Some(factor) = factor_from_qfont(&font) {
                if tx.send(factor).is_err() {
                    return; // the UI is gone: the app is shutting down
                }
                ctx.request_repaint();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qfont_strings() {
        // Qt 6 format, Plasma's default.
        assert_eq!(factor_from_qfont("Noto Sans,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1"), Some(1.0));
        // Qt 5 format, larger.
        assert_eq!(factor_from_qfont("Noto Sans,12,-1,5,50,0,0,0,0,0"), Some(1.2));
        assert_eq!(factor_from_qfont("Inter,10.5,-1,5,400,0,0,0,0,0"), Some(1.05));
        // Pixel-sized: 20 px = 15 pt.
        assert_eq!(factor_from_qfont("Noto Sans,-1,20,5,400,0,0,0,0,0"), Some(1.5));
        // Clamped.
        assert_eq!(factor_from_qfont("Noto Sans,6,-1"), Some(0.75));
        assert_eq!(factor_from_qfont("Noto Sans,48,-1"), Some(3.0));
        // Malformed.
        assert_eq!(factor_from_qfont("Noto Sans"), None);
        assert_eq!(factor_from_qfont("Noto Sans,big,-1"), None);
        assert_eq!(factor_from_qfont("Noto Sans,-1,-1"), None);
    }
}