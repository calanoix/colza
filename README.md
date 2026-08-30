# colorpick

Minimal CLI proof-of-concept for a Wayland-native color tool:

- pick a color from anywhere on screen
- compare two colors
- check WCAG 2.x contrast (AA / AAA, normal or large text)

## Why xdg-desktop-portal instead of zwlr_screencopy

`zwlr_screencopy_manager_v1` is a **wlroots-only** Wayland protocol
extension (Sway, Hyprland, etc.). GNOME and KDE do not implement it, so any
app built on it only works on wlroots compositors.

This project instead uses `org.freedesktop.portal.Screenshot`'s `PickColor`
method, part of the standard [XDG Desktop Portal](https://flatpak.github.io/xdg-desktop-portal/)
spec. The compositor itself draws the eyedropper/crosshair and asks the
user to click a pixel; the app just receives the resulting RGB value. This
works out of the box on:

- GNOME (via `xdg-desktop-portal-gnome`)
- KDE Plasma (via `xdg-desktop-portal-kde`)
- wlroots compositors (via `xdg-desktop-portal-wlr`)

No special permissions, no PipeWire, no `input` group membership needed for
this milestone.

## Requirements

- `xdg-desktop-portal` running, with a backend for your desktop
  (`xdg-desktop-portal-gnome`, `-kde`, or `-wlr`) installed and active.
- Rust (stable), `cargo`.

## Build & run

```sh
cargo build --release

# Click a pixel on screen, print its color
./target/release/colorpick pick

# Compare two known hex colors
./target/release/colorpick compare '#1A2B3C' '#FFFFFF'
./target/release/colorpick compare '#1A2B3C' '#FFFFFF' --large-text

# Click two pixels on screen and compare them
./target/release/colorpick pick-and-compare
```

## Tests

Color math (hex parsing, WCAG contrast ratio, level thresholds) is unit
tested and has no Wayland/portal dependency:

```sh
cargo test
```

## Architecture

```
src/
  color.rs   Rgb type, hex <-> RGB, WCAG relative luminance & contrast ratio
  portal.rs  thin wrapper around ashpd's Screenshot::PickColor portal call
  main.rs    clap CLI: pick / compare / pick-and-compare
```

## Roadmap / milestone 2: live magnifier

`PickColor` is a single click, driven entirely by the compositor: there is
no way to show a custom live-updating magnifier loupe following the real
mouse cursor through a portal alone, because Wayland deliberately does not
expose global cursor position to clients (anti-keylogging/anti-fingerprint
design).

A follow-up milestone for a live magnifier + arrow-key pixel nudging needs:

- **`ashpd::desktop::screencast` (ScreenCast portal) + PipeWire** to get a
  live video frame of the screen, decoded pixel-by-pixel.
- **evdev** (`/dev/input/eventX`, requires the user to be in the `input`
  group — a one-time `sudo usermod -aG input $USER`) to read real mouse
  deltas, since no portal exposes passive cursor position either.
- Optionally, **`RemoteDesktop` portal** (`notify_pointer_motion`) to
  actually move the system cursor in response to arrow-key input, once a
  GUI/overlay exists to show visual feedback (layer-shell on wlroots does
  not have a GNOME/KDE equivalent, so a portable overlay is its own
  design problem).

This keeps milestone 1 fully portable and low-privilege, and isolates the
higher-privilege, wlroots-influenced pieces to a clearly separate, opt-in
milestone.

## License

MIT
