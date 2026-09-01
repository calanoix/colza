//! On-disk storage for the ScreenCast portal's restore token.
//!
//! `PersistMode` alone doesn't stop the compositor from asking "which
//! screen do you want to share?" on every launch — persistence is a
//! two-part handshake: request it via `select_sources(..., persist_mode)`,
//! then pass the `restore_token` that `start()` returns back into the next
//! `select_sources` call to skip the dialog. Skipping that second part
//! leaves the grant unnamed server-side, so the dialog appears every time
//! anyway. Hence this module.
//!
//! The token is single-use: every successful `start()` returns a new token
//! and invalidates the one that was sent. [`store`] must therefore run on
//! every `start()` response, not just when the file is missing, or the
//! dialog reappears on every second run.
//!
//! Nothing here returns an error to the caller. A missing, unreadable,
//! corrupt or stale token just means the user sees the picker dialog once
//! more and a fresh token gets saved from that run. Write failures are
//! reported to stderr and otherwise ignored.

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/colza/screencast-restore-token`, falling back to
/// `$HOME/.config/...` per the XDG basedir spec. `None` if neither
/// variable is set, in which case the app simply operates tokenless.
fn token_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(dir.join("colza").join("screencast-restore-token"))
}

/// The token saved by a previous run, if any.
///
/// Whitespace is trimmed because the file is plain text a user might
/// plausibly edit or `echo` into, and a trailing newline would otherwise
/// be sent to the portal as part of the token and silently invalidate it.
pub fn load() -> Option<String> {
    let raw = std::fs::read_to_string(token_path()?).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Saves the token returned by `start()`, replacing any previous one.
pub fn store(token: &str) {
    let Some(path) = token_path() else {
        return;
    };

    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!("colza: could not create {}: {err}", parent.display());
            return;
        }
    }

    if let Err(err) = std::fs::write(&path, token) {
        eprintln!(
            "colza: could not save the screencast restore token to {}: {err} \
             (the screen-share dialog will appear again next run)",
            path.display()
        );
    }
}