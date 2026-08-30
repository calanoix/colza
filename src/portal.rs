//! Wraps two `org.freedesktop.portal.Screenshot` requests:
//! - `PickColor`, used by the CLI `pick` subcommand (compositor draws its
//!   own eyedropper, we just get back one RGB value).
//! - `Screenshot`, used by the magnifier GUI (we get back a full-screen PNG
//!   we decode and zoom into ourselves, following the pointer inside our
//!   own fullscreen window — the same trick `shmooz` and the old PyQt
//!   `picker.py` used, just backed by the portal instead of
//!   `zwlr_screencopy_manager_v1` or a shelled-out `grim`/`spectacle`).
//!
//! Both work on GNOME and KDE without any wlroots-specific protocol and
//! without any special permissions (no `input` group, no PipeWire).

use crate::color::Rgb;
use image::RgbImage;

/// Ask the desktop portal to let the user click a pixel anywhere on screen
/// and return its color. Blocks (asynchronously) until the user clicks or
/// cancels the compositor-drawn picker.
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
/// in-memory RGB image. `interactive(false)` skips the compositor's
/// area/screen-selection dialog and captures immediately (closest
/// equivalent to the old `spectacle -b -f -n` / `grim` behavior).
pub async fn capture_screen() -> anyhow::Result<RgbImage> {
    let response = ashpd::desktop::screenshot::Screenshot::request()
        .interactive(false)
        .modal(true)
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