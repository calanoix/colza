//! On-disk storage for the ScreenCast portal's *restore token*.
//!
//! ## What this buys us
//!
//! `PersistMode` alone does not stop the compositor from asking "which
//! screen do you want to share?" on every launch. The portal's persistence
//! is a two-part handshake:
//!
//! 1. Ask for persistence — `select_sources(..., persist_mode)` with
//!    something other than `DoNot`.
//! 2. `start()`'s response then carries a `restore_token`. Pass that token
//!    back to `select_sources` next time and the portal restores the
//!    previous selection *without showing the dialog*.
//!
//! Skip step 2 and the grant exists server-side but we have no way to name
//! it, so we get the dialog anyway. Hence this module: somewhere to keep
//! the token between runs.
//!
//! ## The token is single-use
//!
//! Every successful `start()` returns a **new** token and invalidates the
//! one we sent. So the token must be re-saved after each capture, not just
//! written once — [`store`] is called on every `start()` response, not only
//! when the file is missing. Getting this wrong yields the confusing
//! symptom of the dialog reappearing on exactly every second run.
//!
//! ## Failure is always non-fatal
//!
//! Nothing here returns an error to the caller. A missing, unreadable,
//! corrupt or stale token simply means the user sees the picker dialog
//! once more and we save a fresh token from that run — which is the same
//! state a first-ever launch is in. Making capture fail because a cache
//! file could not be written would be a much worse outcome than showing a
//! dialog, so write errors are reported to stderr and otherwise ignored.

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/colza/screencast-restore-token`, falling back to
/// `$HOME/.config/...` per the XDG basedir spec. Returns `None` if neither
/// variable is set, in which case we simply operate tokenless.
fn token_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        // `XDG_CONFIG_HOME` unset *or empty* both mean "use the default",
        // per the spec — an empty value is explicitly to be treated as
        // absent, which `var_os` alone would not do.
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(dir.join("colza").join("screencast-restore-token"))
}

/// The token saved by a previous run, if any.
///
/// Whitespace is trimmed because the file is plain text a user might
/// plausibly edit or `echo` into, and a trailing newline would otherwise be
/// sent to the portal as part of the token and silently invalidate it.
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

    // The parent may well not exist on a first run; `create_dir_all` is a
    // no-op when it does.
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
