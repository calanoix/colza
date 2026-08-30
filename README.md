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

## Milestone 2: fullscreen pixel-zoom magnifier

`colorpick magnify` opens a fullscreen window showing a 9×9 pixel-zoomed
loupe that follows the mouse, with a live hex readout — a Rust/egui port
of an earlier PyQt6 prototype (`PickerOverlay` in `picker.py`).

**Design.** No portal exposes live global cursor position or a
live-updating video feed through a single simple call, so instead of
reaching for evdev + ScreenCast/PipeWire, this reuses a much simpler and
more portable trick (also used by `shmooz`, and by the original PyQt6
prototype):

1. Take one full-screen screenshot via the `Screenshot` portal
   (`ashpd::desktop::screenshot::Screenshot`, non-interactive).
2. Open a **fullscreen window** covering the entire screen.
3. Because the window covers the whole screen, its normal mouse-move
   events are already "global" cursor tracking — no evdev, no `input`
   group, no elevated permissions.
4. Zoom into the captured image around the cursor position each frame.

The window is a standard `xdg_toplevel` put into fullscreen (via
`eframe`/`egui`, which wraps `winit`), not a wlroots layer-shell overlay:
it behaves like a normal application going fullscreen (think pressing F11
in a browser) rather than a furtive system overlay. That's the deliberate
tradeoff for GNOME/KDE portability — see "Fullscreen window vs. overlay"
below.

**Fullscreen window vs. overlay.** `shmooz` uses
`zwlr_layer_shell_v1::Layer::Overlay`, which isn't a window at all in the
usual sense: no title bar, never in Alt-Tab, floats above literally
everything — but it's wlroots-only (Sway, Hyprland, ...), not implemented
by GNOME or KDE. `colorpick magnify` instead uses a normal `xdg_toplevel`
put into fullscreen, which is portable but behaves like a real
application window that happens to be fullscreen (it can appear briefly
with decorations before going fullscreen depending on the compositor, and
does show up in Alt-Tab). That's the deliberate tradeoff for GNOME/KDE
portability.

**Not yet implemented:** moving the *real* system cursor with arrow keys
(the old prototype's `QCursor.setPos`). The window already reads arrow
keys are free to wire up, but actually relocating the system pointer
needs either the `RemoteDesktop` portal or a crate like `enigo` — left for
a follow-up so the base magnifier ships first.

## Requirements

- `xdg-desktop-portal` running, with a backend for your desktop
  (`xdg-desktop-portal-gnome`, `-kde`, or `-wlr`) installed and active.
- Rust (stable), `cargo`.
- System packages for native GUI windowing (`eframe`/`winit`), e.g. on
  Debian/Ubuntu:
  ```sh
  sudo apt install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
      libxkbcommon-dev libssl-dev libwayland-dev libgl1-mesa-dev libegl1-mesa-dev
  ```
  (only needed to *compile*; the `pick`/`compare` subcommands alone don't
  need a GUI toolkit, but since they're built into the same binary as
  `magnify`, these are required for `cargo build` regardless of which
  subcommand you plan to use.)

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

# Fullscreen pixel-zoom magnifier: move mouse to preview,
# left-click or Enter to pick, Esc to cancel
./target/release/colorpick magnify
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
  color.rs      Rgb type, hex <-> RGB, WCAG relative luminance & contrast ratio
  portal.rs     ashpd wrappers: Screenshot::PickColor (pick) and
                Screenshot::request (capture_screen, for the magnifier)
  magnifier.rs  eframe/egui fullscreen magnifier window (picker.py port)
  main.rs       clap CLI: pick / compare / pick-and-compare / magnify
```

Note on `main.rs`: `magnify` cannot run inside `#[tokio::main]` like the
other subcommands, because `eframe`'s windowing backend needs to own the
real OS main thread on Linux. `main` builds a short-lived tokio runtime
just for the one async portal call, then runs `eframe` synchronously on
the same (main) thread afterwards.

## Roadmap

- Move the real system cursor with arrow keys inside the magnifier
  (`RemoteDesktop` portal or `enigo`).
- Wire the magnifier's picked color into `compare`/`pick-and-compare` so
  the GUI and CLI flows can be combined.

## License

MIT