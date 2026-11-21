//! Still screen capture via `org.freedesktop.portal.Screenshot`.
//!
//! [`capture_screen`] is the fallback capture path, used when the user
//! declines the ScreenCast permission (see `screencast::open_best`): a
//! full-screen PNG that colza decodes and zooms into itself.
//!
//! `Screenshot` has no `cursor_mode` option, so its image always has the
//! system cursor baked in and never updates. `screencast.rs`'s
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
///
/// Each answer is also recorded for the settings page (see
/// `permissions.rs`): sandboxed, what these requests return is the only
/// sign colza gets of the permission the portal stored.
pub async fn capture_screen() -> anyhow::Result<RgbImage> {
    let response = ashpd::desktop::screenshot::Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await?
        .response();
    // A stored "no", or "Deny" in the portal's question, comes back as
    // response code 2 ("other"), not 1 ("cancelled"): xdg-desktop-portal
    // answers for the backend without calling it. A backend that fails
    // for some other reason answers 2 as well, so this can occasionally
    // read a failure as a refusal; the settings page then just offers a
    // reset that wasn't needed.
    match &response {
        Ok(_) => crate::permissions::record_outcome(crate::permissions::Answer::Allowed),
        Err(err) if is_refusal_error(err) => {
            crate::permissions::record_outcome(crate::permissions::Answer::Denied)
        }
        Err(_) => {}
    }
    let response = response?;

    let uri = response.uri();
    // ashpd 0.13's `Uri` is a lightweight string wrapper with no file-path
    // conversion of its own (it replaced the old `url::Url`-backed type),
    // so `url`, already a dependency, does the `file://` → path parsing.
    let parsed = url::Url::parse(uri.as_str())
        .map_err(|err| anyhow::anyhow!("portal returned an unparseable URI '{uri}': {err}"))?;
    let path = parsed
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("portal returned a non-local URI: {uri}"))?;

    let image = image::open(&path)
        .map_err(|err| anyhow::anyhow!("failed to decode screenshot at {path:?}: {err}"))?
        .to_rgb8();

    // Best-effort cleanup: the portal writes into a per-session temp/cache
    // dir, so a failure here isn't fatal to the caller.
    let _ = std::fs::remove_file(&path);

    Ok(image)
}
/// Whether a Screenshot request failed because the permission was
/// refused (now, or by an answer the portal stored) rather than because
/// something broke. See the note in [`capture_screen`] on why "other"
/// counts.
fn is_refusal_error(err: &ashpd::Error) -> bool {
    matches!(
        err,
        ashpd::Error::Response(
            ashpd::desktop::ResponseError::Other | ashpd::desktop::ResponseError::Cancelled
        )
    )
}