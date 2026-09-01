//! Wraps the two `org.freedesktop.portal.Screenshot` requests this app
//! uses:
//!
//! - [`pick_color`] (`PickColor`), used by the CLI `pick` subcommand: the
//!   compositor draws its own eyedropper and we just get back one RGB
//!   value.
//! - [`capture_screen`] (`Screenshot`), the **fallback** capture path: we
//!   get back a full-screen PNG, decode it, and zoom into it ourselves.
//!
//! ## Why `capture_screen` exists again
//!
//! It used to be the only capture path, then `screencast.rs` replaced it
//! because `Screenshot` has no `cursor_mode` knob — the system cursor is
//! always baked into the returned image, which looked wrong under the
//! magnifier's own crosshair.
//!
//! It is back as the degraded mode for one specific case: the user
//! declining the screen-share prompt. `ScreenCast` cannot work without that
//! grant, and `Screenshot` at least keeps the magnifier working (a still
//! image instead of a live feed, cursor artifact included) rather than
//! losing the zoom entirely. See `screencast::open_best`.
//!
//! Note that "fallback" does not mean "unprivileged": `Screenshot` has its
//! own persistent entry in the portal permission store, separate from
//! screencast's. It is commonly pre-granted for non-sandboxed apps, which
//! is why it usually goes through silently, but that is a property of the
//! user's portal config rather than a guarantee.

use crate::color::Rgb;
use image::RgbImage;

/// Ask the desktop portal to let the user click a pixel anywhere on screen
/// and return its color. Blocks (asynchronously) until the user clicks or
/// cancels the compositor-drawn picker.
///
/// This is the lowest-privilege path in the app: the compositor samples the
/// pixel itself and hands back three floats, so no screen contents ever
/// reach this process and nothing persistent is granted.
pub async fn pick_color() -> anyhow::Result<Rgb> {
    let color = ashpd::desktop::Color::pick()
        .send()
        .await?
        .response()?;

    Ok(Rgb::from_unit_floats(
        color.red(),
        color.green(),
        color.blue(),
    ))
}

/// Ask the desktop portal for a full-screen screenshot and decode it to an
/// in-memory RGB image.
///
/// `interactive(false)` skips the compositor's area/screen-selection dialog
/// and captures immediately (closest equivalent to the old `spectacle -b -f
/// -n` / `grim` behaviour).
///
/// The returned image **includes the mouse cursor**, which this interface
/// gives no way to suppress. Callers showing it under the magnifier should
/// expect an arrow baked into the pixels near the pointer.
pub async fn capture_screen() -> anyhow::Result<RgbImage> {
    let response = ashpd::desktop::screenshot::Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await?
        .response()?;

    let uri = response.uri();
    let path = uri
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("portal returned a non-local URI: {uri}"))?;

    let img = image::open(&path)
        .map_err(|err| anyhow::anyhow!("failed to decode screenshot at {path:?}: {err}"))?
        .to_rgb8();

    // Best-effort cleanup; the portal writes into a per-session temp/cache
    // dir, so a failure here isn't fatal to the caller.
    let _ = std::fs::remove_file(&path);

    Ok(img)
}
