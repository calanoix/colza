//! Wraps `org.freedesktop.portal.Screenshot`'s `PickColor` request, used by
//! the CLI `pick` subcommand (compositor draws its own eyedropper, we just
//! get back one RGB value).
//!
//! This is intentionally the *only* thing left in this module. Full-screen
//! capture (used by the magnifier GUI) used to live here too, backed by
//! `Screenshot::request()`, but was moved to `screencast.rs` — see that
//! module's doc comment for why (short version: `Screenshot` has no
//! `cursor_mode` option, so the system cursor was always baked into the
//! captured image; `ScreenCast` + PipeWire does support hiding it).
//! `PickColor` and `Screenshot` are different D-Bus interfaces under the
//! same portal umbrella, and `PickColor` has no such cursor artifact to
//! begin with (the compositor returns a single sampled color, not an
//! image), so there was no reason to move it.

use crate::color::Rgb;

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