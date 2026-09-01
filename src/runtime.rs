//! One tokio runtime for the whole process.
//!
//! ashpd caches its D-Bus connection in a process-wide static, so every
//! portal call after the first reuses the same connection. With ashpd's
//! `tokio` feature, that connection's socket is registered with the
//! reactor of whichever tokio runtime happened to be current when it was
//! created — so the runtime must outlive the connection, which in practice
//! means outliving the process. Building and dropping a short-lived
//! runtime per call breaks on the *second* call: the cached connection
//! keeps pointing at the first runtime's now-dead reactor, causing
//! "there is no reactor running" panics.
//!
//! The runtime is multi-threaded rather than `current_thread` for two
//! reasons: its reactor and timer keep running on worker threads between
//! calls instead of only while someone is inside `block_on`, and it has a
//! blocking pool that `screencast.rs` uses for PipeWire's blocking
//! mainloop.

use std::sync::OnceLock;
use tokio::runtime::Runtime;

/// The process-wide runtime, built on first use.
///
/// Safe to call from any thread, and safe to `block_on` from several
/// threads at once. Do NOT call it from inside an async task and then
/// `block_on` — that deadlocks, as with any runtime.
pub fn shared() -> anyhow::Result<&'static Runtime> {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();

    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }

    // Built outside `get_or_init` so a failure to build can be reported as
    // an error instead of panicking inside the initializer. If another
    // thread wins the race, this one's runtime is dropped here having
    // never run a task, which is fine and only legal because we are not
    // inside an async context.
    let rt = Runtime::new()?;
    Ok(RUNTIME.get_or_init(|| rt))
}