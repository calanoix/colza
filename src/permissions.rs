//! The screenshot permission: reading and resetting the answer the portal
//! saved, for the Permissions card on the settings page.
//!
//! The two portals colza uses remember a "no" in very different places,
//! which is why the settings page treats them differently:
//!
//! - **ScreenCast** stores no refusal at all. Each request either restores
//!   a saved grant (the restore token) or shows the picker again. The only
//!   "denied" state is colza's own, for this run
//!   (`screencast::was_declined`), so colza can always undo it itself.
//!
//! - **Screenshot**, non-interactive (what colza asks for), is gated by
//!   xdg-desktop-portal itself, on backends implementing version 2 of its
//!   Screenshot interface (GNOME's does): the first request shows an
//!   "Allow colza to take screenshots?" dialog, and the answer is stored
//!   for good in the permission store, table `screenshot`, id
//!   `screenshot`, under the app's id. A stored "no" then makes every
//!   request fail at once, with no dialog and response code 2 ("other"):
//!   the portal offers apps no way to ask again (checked against
//!   `desktop-portal/screenshot.c`, `check_non_interactive_permission_in_thread`).
//!   The only way back is deleting that entry.
//!
//! The permission store (`org.freedesktop.impl.portal.PermissionStore`)
//! is a plain session-bus service, reachable from a host install, where
//! colza reads and resets the entry itself. Flatpak's D-Bus proxy blocks
//! it, and on purpose: talking to it would mean reading and rewriting
//! every app's permissions (`--talk-name` for it would be a sandbox hole
//! Flathub would rightly refuse). There, colza can only report what its
//! own requests returned ([`last_outcome`]) and show the user the
//! `flatpak permission-remove` command that resets the entry.

use std::sync::atomic::{AtomicU8, Ordering};

use ashpd::zbus;

const STORE_NAME: &str = "org.freedesktop.impl.portal.PermissionStore";
const STORE_PATH: &str = "/org/freedesktop/impl/portal/PermissionStore";
const STORE_INTERFACE: &str = "org.freedesktop.impl.portal.PermissionStore";
/// Table and resource id the Screenshot portal stores its answer under
/// (`SCREENSHOT_PERMISSION_TABLE` and `_ID` in xdg-desktop-portal).
const TABLE: &str = "screenshot";
const ID: &str = "screenshot";
/// What the store returns for a table or id with no entry at all, which
/// just means no answer was saved yet.
const NOT_FOUND: &str = "org.freedesktop.portal.Error.NotFound";

/// An answer to the screenshot question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    /// Nothing saved: the next screenshot may show the question (on
    /// backends that ask at all).
    Unset,
    Allowed,
    Denied,
}

/// The stored answer, as far as colza can see it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lookup {
    /// Read from the permission store (host installs).
    Stored(Answer),
    /// The store can't be reached (sandboxed): this is what colza's own
    /// last screenshot request returned this run, if it made one.
    Observed(Option<Answer>),
}

impl Lookup {
    pub fn answer(self) -> Option<Answer> {
        match self {
            Self::Stored(answer) => Some(answer),
            Self::Observed(answer) => answer,
        }
    }
}

/// The outcome of this run's latest screenshot request: 0 = none yet,
/// 1 = allowed, 2 = denied. Written by `portal::capture_screen`.
static LAST_OUTCOME: AtomicU8 = AtomicU8::new(0);

/// Records what a non-interactive screenshot request just returned.
pub fn record_outcome(answer: Answer) {
    let value = match answer {
        Answer::Unset => 0,
        Answer::Allowed => 1,
        Answer::Denied => 2,
    };
    LAST_OUTCOME.store(value, Ordering::Relaxed);
}

/// What this run's latest screenshot request returned, if any.
pub fn last_outcome() -> Option<Answer> {
    match LAST_OUTCOME.load(Ordering::Relaxed) {
        1 => Some(Answer::Allowed),
        2 => Some(Answer::Denied),
        _ => None,
    }
}

/// Whether colza runs sandboxed, where the store is out of reach.
pub fn is_sandboxed() -> bool {
    ashpd::is_sandboxed()
}

/// The command that resets the stored answer from outside the sandbox,
/// or `None` when not running as a Flatpak (then colza does it itself).
pub fn reset_command() -> Option<String> {
    let app_id = std::env::var("FLATPAK_ID").ok().filter(|id| !id.is_empty())?;
    Some(format!("flatpak permission-remove {TABLE} {ID} {app_id}"))
}

/// The app ids the portal may have stored colza's answer under, for a
/// host (unsandboxed) install.
///
/// The portal can't know a host app's id for sure. It takes it from the
/// systemd scope the process runs in, which is named after the desktop
/// file when colza is launched from a menu (hence `APP_ID`, which names
/// colza's `.desktop` file), and otherwise falls back to "" (launched
/// from a terminal). Which one applied depends on how this run started,
/// so both are read and both are reset.
///
/// "" is shared by every unsandboxed app the portal can't identify, so
/// resetting it makes those ask again too. That is the portal's own
/// granularity (its dialog for "" even says "Allow apps to take
/// screenshots?"), and a reset only ever leads to a question, never to a
/// silent grant.
fn host_keys() -> [&'static str; 2] {
    [crate::app::APP_ID, ""]
}

/// Reads the stored answer; [`Lookup::Observed`] when the store can't be
/// reached. Must be awaited on `runtime::shared()`.
pub async fn lookup() -> Lookup {
    match read_store().await {
        Ok(answer) => Lookup::Stored(answer),
        Err(err) => {
            if !is_sandboxed() {
                eprintln!("colza: could not read the screenshot permission: {err}");
            }
            Lookup::Observed(last_outcome())
        }
    }
}

async fn read_store() -> zbus::Result<Answer> {
    if is_sandboxed() {
        // Don't even try: the proxy would refuse, and the refusal is
        // expected rather than worth an error.
        return Err(zbus::Error::Unsupported);
    }
    let connection = zbus::Connection::session().await?;
    let mut answers = Vec::new();
    for app in host_keys() {
        let reply = connection
            .call_method(
                Some(STORE_NAME),
                STORE_PATH,
                Some(STORE_INTERFACE),
                "GetPermission",
                &(TABLE, ID, app),
            )
            .await;
        let permissions: Vec<String> = match reply {
            Ok(message) => message.body().deserialize()?,
            Err(err) if is_not_found(&err) => Vec::new(),
            Err(err) => return Err(err),
        };
        answers.push(match permissions.first().map(String::as_str) {
            Some("yes") => Answer::Allowed,
            Some("no") => Answer::Denied,
            // "ask" (ask every time, set by hand) saves nothing to reset.
            _ => Answer::Unset,
        });
    }
    // Which key this run uses can't be known (see `host_keys`), so a "no"
    // under either is reported: it's the one that can block, and the one
    // a reset fixes.
    Ok(if answers.contains(&Answer::Denied) {
        Answer::Denied
    } else if answers.contains(&Answer::Allowed) {
        Answer::Allowed
    } else {
        Answer::Unset
    })
}

/// Deletes the stored answer, so the next screenshot asks again. Fails
/// when sandboxed: use [`reset_command`] there. Must be awaited on
/// `runtime::shared()`.
pub async fn reset() -> anyhow::Result<()> {
    if is_sandboxed() {
        anyhow::bail!("the permission store isn't reachable from the sandbox");
    }
    let connection = zbus::Connection::session().await?;
    for app in host_keys() {
        let reply = connection
            .call_method(
                Some(STORE_NAME),
                STORE_PATH,
                Some(STORE_INTERFACE),
                "DeletePermission",
                &(TABLE, ID, app),
            )
            .await;
        match reply {
            Ok(_) => {}
            // Nothing stored under this key: already reset.
            Err(err) if is_not_found(&err) => {}
            Err(err) => return Err(err.into()),
        }
    }
    // What colza saw earlier this run no longer says anything.
    record_outcome(Answer::Unset);
    Ok(())
}

fn is_not_found(err: &zbus::Error) -> bool {
    matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == NOT_FOUND)
}