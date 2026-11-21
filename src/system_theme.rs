//! Follows the desktop's light/dark preference, for the "System" theme.
//!
//! egui gets the system theme from winit, which on Wayland doesn't know
//! it: winit 0.30's `Window::theme()` there only returns the theme of its
//! own client-side decorations, `None` unless set. egui then falls back
//! to `Options::fallback_theme`, which defaults to dark — so on a light
//! Plasma (Breeze), colza came up dark. (X11 is no better: winit returns
//! `None` there too.)
//!
//! The preference comes instead from the Settings portal's
//! `org.freedesktop.appearance` `color-scheme` key (0 = no preference,
//! 1 = prefer dark, 2 = prefer light), which `xdg-desktop-portal-kde`
//! derives from the Plasma color scheme and the GNOME backend from its
//! style setting. It's applied as egui's *fallback* theme, so it only
//! decides when winit has nothing to say, and a theme forced in colza's
//! settings still wins.
//!
//! "No preference" and every failure (no portal, a backend without the
//! key) mean light: that's what a desktop with no stated preference shows
//! by default.

use std::time::Duration;

use ashpd::desktop::settings::{ColorScheme, Settings};
use ashpd::zvariant::Value;
use eframe::egui;
use futures_util::StreamExt;

const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

/// Same cap as `text_scale::read_blocking`, for the same reason: a local
/// D-Bus round-trip normally, but a hung portal must not hold the window.
const STARTUP_TIMEOUT: Duration = Duration::from_millis(500);

fn theme_for(scheme: ColorScheme) -> egui::Theme {
    match scheme {
        ColorScheme::PreferDark => egui::Theme::Dark,
        ColorScheme::PreferLight | ColorScheme::NoPreference => egui::Theme::Light,
    }
}

/// The value of `color-scheme`, as sent in a change signal.
fn theme_for_value(value: &Value) -> Option<egui::Theme> {
    let n = match value {
        Value::U32(n) => *n,
        // One more variant layer, as `text_scale::watch` also tolerates.
        Value::Value(inner) => match &**inner {
            Value::U32(n) => *n,
            _ => return None,
        },
        _ => return None,
    };
    Some(if n == 1 { egui::Theme::Dark } else { egui::Theme::Light })
}

/// The desktop's preference, read once at startup so the first frame
/// already has the right colors. Light on any failure; blocks for at most
/// [`STARTUP_TIMEOUT`].
///
/// Must be called outside any async context (`runtime::shared` rule).
pub fn read_blocking() -> egui::Theme {
    let Ok(rt) = crate::runtime::shared() else {
        return egui::Theme::Light;
    };
    rt.block_on(async {
        tokio::time::timeout(STARTUP_TIMEOUT, async {
            let settings = Settings::new().await.ok()?;
            settings.color_scheme().await.ok()
        })
        .await
        .ok()
        .flatten()
        .map_or(egui::Theme::Light, theme_for)
    })
}

/// Makes `theme` the theme egui uses when following the system.
pub fn apply(ctx: &egui::Context, theme: egui::Theme) {
    ctx.options_mut(|o| o.fallback_theme = theme);
}

/// Applies the preference again whenever the user switches between light
/// and dark color schemes, and repaints so it shows at once.
///
/// Runs for the life of the process on the shared runtime. If the portal
/// is unavailable it simply never fires; startup already fell back.
pub fn watch(ctx: egui::Context) {
    let Ok(rt) = crate::runtime::shared() else {
        return;
    };
    rt.spawn(async move {
        let Ok(settings) = Settings::new().await else {
            return;
        };
        // Unfiltered and parsed here, as in `text_scale::watch`.
        let Ok(mut changes) = settings.receive_setting_changed().await else {
            return;
        };
        while let Some(setting) = changes.next().await {
            if setting.namespace() != NAMESPACE || setting.key() != KEY {
                continue;
            }
            if let Some(theme) = theme_for_value(&**setting.value()) {
                apply(&ctx, theme);
                ctx.request_repaint();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes() {
        assert_eq!(theme_for(ColorScheme::PreferDark), egui::Theme::Dark);
        assert_eq!(theme_for(ColorScheme::PreferLight), egui::Theme::Light);
        assert_eq!(theme_for(ColorScheme::NoPreference), egui::Theme::Light);
        assert_eq!(theme_for_value(&Value::U32(1)), Some(egui::Theme::Dark));
        assert_eq!(theme_for_value(&Value::U32(2)), Some(egui::Theme::Light));
        assert_eq!(theme_for_value(&Value::U32(0)), Some(egui::Theme::Light));
        assert_eq!(
            theme_for_value(&Value::Value(Box::new(Value::U32(1)))),
            Some(egui::Theme::Dark)
        );
        assert_eq!(theme_for_value(&Value::Str("dark".into())), None);
    }
}