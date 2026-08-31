//! One tokio runtime for the whole process.
//!
//! ## Why this module exists
//!
//! ashpd caches its D-Bus connection in a process-wide
//! `static SESSION: OnceLock<zbus::Connection>` (ashpd 0.9's `proxy.rs`),
//! so every portal call after the first reuses the *same* connection. With
//! ashpd's `tokio` feature, zbus registers that connection's socket with
//! the reactor of whichever tokio runtime happened to be current when it
//! was created.
//!
//! The app used to build a fresh short-lived `current_thread` runtime per
//! portal call and drop it when the call returned. That was harmless while
//! ashpd ran on its default `async-std` backend, whose `async-io` reactor
//! is a global independent of any runtime. Once Cargo.toml switched ashpd
//! to `tokio` (it refuses both backends at once), the pattern broke in a
//! way that only showed up on the *second* call:
//!
//! ```text
//! thread '<unnamed>' panicked at zbus-4.4.0/src/abstractions/executor.rs:189:27:
//! there is no reactor running, must be called from the context of a Tokio 1.x runtime
//! ```
//!
//! First pick: runtime #1 is created, the cached connection binds to its
//! reactor, the pick succeeds, runtime #1 is dropped. Second pick: runtime
//! #2 is current, but the cached connection still points at runtime #1's
//! now-dead reactor — panic.
//!
//! So the runtime must outlive the connection, which means outliving the
//! process. Hence a `'static` one, created once.
//!
//! ## Why multi-threaded
//!
//! Two reasons, beyond it being the default shape for a long-lived
//! runtime:
//!
//! 1. Its reactor and timer live on worker threads it owns, so they keep
//!    running between calls — a `current_thread` runtime only drives them
//!    while someone is inside `block_on`, which is exactly the "no reactor
//!    running" hazard above in slow motion.
//! 2. It has a blocking pool, so `screencast.rs` can hand PipeWire's
//!    blocking `mainloop.run()` to `spawn_blocking` instead of hand-rolling
//!    a thread plus a polling bridge back to async.

use std::sync::OnceLock;
use tokio::runtime::Runtime;

/// The process-wide runtime, built on first use.
///
/// Safe to call from any thread, and safe to `block_on` from several
/// threads at once: `block_on` parks the calling thread while the runtime's
/// own workers keep driving the reactor. Do NOT call it from *inside* an
/// async task and then `block_on` — that deadlocks, as it would with any
/// runtime.
pub fn shared() -> anyhow::Result<&'static Runtime> {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();

    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }

    // Built outside `get_or_init` so a failure to build can be reported as
    // an error instead of panicking inside the initializer. If another
    // thread wins the race, this one's runtime is dropped here having never
    // run a task — which is fine, and only legal because we are not inside
    // an async context (dropping a runtime from one panics).
    let rt = Runtime::new()?;
    Ok(RUNTIME.get_or_init(|| rt))
}
