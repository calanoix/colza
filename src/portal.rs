//! Wraps the two `org.freedesktop.portal.Screenshot` requests this app
//! uses:
//!
//! - [`pick_color`] (`PickColor`), used by the CLI `pick` subcommand: the
//!   compositor draws its own eyedropper and we just get back one RGB
//!   value.
//! - [`capture_screen`] (`Screenshot`), the fallback capture path used when
//!   the user declines the ScreenCast permission (see
//!   `screencast::open_best`): a full-screen PNG we decode and zoom into
//!   ourselves.
//!
//! `Screenshot` has no `cursor_mode` knob, so its image always has the
//! system cursor baked in and never updates — `screencast.rs`'s
//! PipeWire-based capture is used instead whenever the user grants it.

use image::RgbImage;

/// Asks the desktop portal for a full-screen screenshot and decodes it to
/// an in-memory RGB image.
///
/// `interactive(false)` skips the compositor's area/screen-selection
/// dialog and captures immediately.
///
/// The returned image includes the mouse cursor, which this interface
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
    // ashpd 0.13's `Uri` is a lightweight string wrapper with no file-path
    // conversion of its own (it replaced the old `url::Url`-backed type),
    // so `url` — already a dependency — does the `file://` -> path parsing.
    let parsed = url::Url::parse(uri.as_str())
        .map_err(|err| anyhow::anyhow!("portal returned an unparseable URI '{uri}': {err}"))?;
    let path = parsed
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