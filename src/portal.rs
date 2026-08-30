//! Wraps the `org.freedesktop.portal.Screenshot` PickColor request.
//!
//! This is the standard, sandbox-friendly way to sample a screen pixel on
//! Wayland: the compositor itself draws the crosshair/eyedropper and asks
//! the user to click, so it works on GNOME and KDE without any
//! wlroots-specific protocol (no `zwlr_screencopy_manager_v1`) and without
//! any special permissions (no `input` group, no PipeWire).
//!
//! The tradeoff versus a custom overlay is interactivity: there is no live
//! magnifier following the mouse, just a single click producing a single
//! color. A follow-up milestone can add a live magnifier via
//! ScreenCast + PipeWire, which does need broader permissions.

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
