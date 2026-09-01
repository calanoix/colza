//! Screen capture via `org.freedesktop.portal.ScreenCast` + PipeWire.
//!
//! `ScreenCast`'s `SelectSourcesOptions` supports `CursorMode::Hidden`,
//! which omits the mouse cursor from the video frames entirely — unlike
//! `Screenshot` (`portal.rs`), which always bakes the cursor into the
//! image. The cost is that `ScreenCast` hands back a raw PipeWire node id
//! + fd instead of a finished image, so this module has to speak PipeWire
//! to pull frames out of it.
//!
//! [`Capture`] keeps the session and the PipeWire stream alive and
//! continuously overwrites a single "latest frame" slot for as long as the
//! handle is held, so the magnifier gets a live feed rather than a frozen
//! snapshot — the loupe tracks a playing video, for example. The UI calls
//! [`Capture::take_frame`] once per repaint and swaps in whatever arrived;
//! dropping the handle stops the stream and closes the portal session.
//!
//! The slot is a `Mutex<Option<RgbImage>>` rather than a channel on
//! purpose: a channel would queue full-screen frames whenever the UI
//! repaints slower than the compositor produces them, which at 4K is tens
//! of megabytes per second of backlog. Overwriting means a slow consumer
//! drops intermediate frames, which is the right behavior when only the
//! newest one has any value.
//!
//! `select_sources` is handed a saved restore token (`token_store.rs`)
//! with `PersistMode::ExplicitlyRevoked`, so the compositor restores the
//! previous screen selection silently instead of prompting on every run.

use crate::token_store;
use ashpd::desktop::{
    screencast::{CursorMode, Screencast, SourceType},
    PersistMode, Session,
};
use ashpd::WindowIdentifier;
use image::RgbImage;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Frames per second requested from the compositor.
///
/// This is the main cost knob for the live feed: every frame is converted
/// from the compositor's layout (usually BGRx) into a packed `RgbImage` on
/// the CPU, so the work scales with `framerate * screen area`. 30 keeps
/// the loupe feeling immediate on a video; lower it if a large display
/// makes the capture thread too hot.
///
/// This is a maximum, not a promise: compositors generally only send a
/// frame when something actually changed, so a static screen costs
/// nothing.
const FRAMERATE: u32 = 30;

/// How long [`Capture::open`] waits for the compositor's first frame
/// before giving up.
///
/// By the time this wait starts, the portal round-trip (and the picker
/// dialog, on runs that show one) is already done, and a compositor sends
/// the current screen contents as soon as the stream connects. Several
/// seconds is generous for the happy path; the timeout exists for the
/// paths that would otherwise hang forever — see
/// [`Capture::no_frame_error`] for what those are.
const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Counters the PipeWire thread bumps as it goes, read only when something
/// goes wrong, so a failure message can say *which* stage failed instead
/// of just "no frame arrived" (whose causes range from a genuine
/// limitation of this module to a sleeping monitor).
///
/// `Relaxed` throughout: these are diagnostics only, never used to
/// synchronize anything. The frame handoff itself goes through the
/// `latest` mutex.
#[derive(Default)]
struct StreamDiagnostics {
    /// `param_changed` successfully parsed a video format.
    format_negotiated: AtomicBool,
    /// Buffers successfully dequeued from the stream.
    buffers_dequeued: AtomicU32,
    /// Buffers whose plane had no CPU-readable data — the DMA-BUF
    /// signature: the compositor handed over GPU memory, which this
    /// module deliberately does not import.
    planes_unmapped: AtomicU32,
    /// Buffers that made it all the way to an `RgbImage`.
    frames_decoded: AtomicU32,
}

/// Remembers, for this process only, that the user declined the
/// screen-share prompt, so later picks fall back to `Screenshot` directly
/// instead of re-prompting someone who already said no.
///
/// Deliberately not persisted to disk, unlike the restore token: a
/// remembered "no" with no visible way to undo it would trap the user into
/// never seeing the live magnifier again without knowing a config file
/// exists. Restarting the app is a discoverable reset; a stale file on
/// disk is not.
static SCREENCAST_DECLINED: AtomicBool = AtomicBool::new(false);

/// Where the magnifier's pixels come from.
///
/// | | [`Self::Live`] | [`Self::Still`] |
/// |---|---|---|
/// | Portal | `ScreenCast` + PipeWire | `Screenshot` |
/// | Updates | continuously | never |
/// | Mouse cursor | hidden | baked into the image |
pub enum ScreenSource {
    /// A running ScreenCast session pushing new frames.
    Live(Capture),
    /// A single `Screenshot` frame, already handed to the caller. Holds
    /// nothing because there is nothing left running.
    Still,
}

impl ScreenSource {
    /// The newest frame, if one has arrived since the last call.
    ///
    /// Always `None` for [`Self::Still`], which is exactly what a caller
    /// doing `if let Some(f) = source.take_frame()` needs: it simply keeps
    /// displaying the image it already has.
    pub fn take_frame(&self) -> Option<RgbImage> {
        match self {
            Self::Live(capture) => capture.take_frame(),
            Self::Still => None,
        }
    }

    /// Whether this source updates. Callers can use it to label the
    /// degraded mode in their UI.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }
}

/// Opens the best screen source available, falling back when the user
/// declines the screen-share prompt.
///
/// Order of attempts:
/// 1. `ScreenCast` — live feed, cursor hidden. Skipped outright if the
///    user already declined once this run.
/// 2. `Screenshot` — a still frame, cursor included.
///
/// Only an explicit refusal triggers the fallback; every other failure
/// propagates. Silently degrading on a technical error would throw away
/// the diagnostics `Capture::no_frame_error` produces, turning something
/// like "DMA-BUF is unsupported on this compositor" into the much
/// harder-to-chase "the magnifier is mysteriously never live".
pub async fn open_best() -> anyhow::Result<(ScreenSource, RgbImage)> {
    if !SCREENCAST_DECLINED.load(Ordering::Relaxed) {
        match Capture::open().await {
            Ok((capture, first)) => return Ok((ScreenSource::Live(capture), first)),
            Err(err) if is_user_refusal(&err) => {
                SCREENCAST_DECLINED.store(true, Ordering::Relaxed);
                eprintln!(
                    "colza: screen sharing declined — falling back to a still screenshot. \
                     The magnifier will not track changes on screen, and the mouse cursor \
                     will appear in the captured image. Restart colza to be asked again."
                );
            }
            Err(err) => return Err(err),
        }
    }

    let still = crate::portal::capture_screen().await?;
    Ok((ScreenSource::Still, still))
}

/// Whether this error is the user saying no, as opposed to something
/// breaking.
///
/// The portal reports a declined request as a normal response carrying
/// `ResponseType::Cancelled`, which ashpd surfaces as
/// `Error::Response(ResponseError::Cancelled)`. This also matches the user
/// dismissing the dialog with Escape rather than clicking a "deny" button;
/// the portal makes no distinction, and neither does this function.
fn is_user_refusal(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<ashpd::Error>(),
        Some(ashpd::Error::Response(
            ashpd::desktop::ResponseError::Cancelled
        ))
    )
}

/// A live ScreenCast session: a portal session, a PipeWire stream, and the
/// thread pumping frames out of it.
///
/// Holding this keeps the capture running. Dropping it stops the stream,
/// joins the thread, and closes the portal session.
pub struct Capture {
    /// The newest frame the PipeWire thread has produced, or `None` if the
    /// consumer has already taken it and no newer one has arrived.
    latest: Arc<Mutex<Option<RgbImage>>>,
    /// Signals the PipeWire mainloop to quit. `pipewire::channel` rather
    /// than `std::sync::mpsc` because the receiving thread is blocked
    /// inside `mainloop.run()` and can only be woken through the loop
    /// itself.
    stop: pipewire::channel::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Kept alive purely so the compositor keeps feeding the stream —
    /// closing the session tears the PipeWire node down. `Option` only so
    /// `Drop` can move it out.
    session: Option<Session<'static, Screencast<'static>>>,
    diagnostics: Arc<StreamDiagnostics>,
}

impl Capture {
    /// Opens a ScreenCast session with the cursor hidden and waits for the
    /// first frame.
    ///
    /// Returns the live handle plus that first frame, so a caller can put
    /// a magnifier on screen with something to show immediately rather
    /// than flashing an empty overlay while negotiation finishes.
    ///
    /// Must be awaited on `runtime::shared()`: ashpd's cached D-Bus
    /// connection binds to the first runtime it sees (see runtime.rs).
    pub async fn open() -> anyhow::Result<(Self, RgbImage)> {
        let proxy: Screencast<'static> = Screencast::new().await?;
        let session = proxy.create_session().await?;

        proxy
            .select_sources(
                &session,
                CursorMode::Hidden,
                SourceType::Monitor.into(),
                // Only ever one output: asking for multiple just adds a
                // "select all the ones you want" step to the picker dialog
                // for no benefit here.
                false,
                // The saved token plus `ExplicitlyRevoked` are what
                // suppress the picker dialog on later runs. `Application`
                // would only persist for the running process, so it would
                // still prompt once per launch.
                token_store::load().as_deref(),
                PersistMode::ExplicitlyRevoked,
            )
            .await?
            .response()?;

        // `WindowIdentifier::default()` (the `None` variant) means the
        // dialog, on runs where it appears, isn't parented to our window —
        // exporting a real handle would need ashpd's wayland/gtk4 features
        // and access to eframe's surface.
        let response = proxy
            .start(&session, &WindowIdentifier::default())
            .await?
            .response()?;

        // Saved immediately: each successful `start()` invalidates the
        // token that was sent, so skipping this re-save is what would make
        // the dialog reappear on every second run.
        if let Some(token) = response.restore_token() {
            token_store::store(token);
        }

        let stream = response
            .streams()
            .first()
            .ok_or_else(|| anyhow::anyhow!("compositor returned no screencast streams"))?;
        let node_id = stream.pipe_wire_node_id();

        let pw_fd = proxy.open_pipe_wire_remote(&session).await?;

        let latest: Arc<Mutex<Option<RgbImage>>> = Arc::new(Mutex::new(None));
        let diagnostics: Arc<StreamDiagnostics> = Arc::default();
        let (stop, stop_rx) = pipewire::channel::channel::<()>();
        // Signals "the first frame is in the slot" or "setup failed". A
        // oneshot rather than an mpsc because `oneshot::Sender::send` is
        // non-blocking and needs no runtime, so the plain OS thread below
        // can fire it directly.
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();

        let thread = std::thread::Builder::new()
            .name("colza-pipewire".into())
            .spawn({
                let latest = Arc::clone(&latest);
                let diagnostics = Arc::clone(&diagnostics);
                move || {
                    let ready: ReadySignal =
                        std::rc::Rc::new(std::cell::RefCell::new(Some(ready_tx)));
                    if let Err(err) = run_stream(
                        pw_fd,
                        node_id,
                        latest,
                        diagnostics,
                        stop_rx,
                        std::rc::Rc::clone(&ready),
                    ) {
                        match ready.borrow_mut().take() {
                            // Failed before the first frame — report it to
                            // `open()`, which is still waiting.
                            Some(tx) => {
                                let _ = tx.send(Err(err));
                            }
                            // Failed mid-stream, long after `open()`
                            // returned. Nobody is listening, so stderr is
                            // the only place left to say so.
                            None => {
                                eprintln!("colza: screencast stream ended with an error: {err}")
                            }
                        }
                    }
                }
            })?;

        // Assembled before waiting for the first frame, so every error
        // path below is a plain `return` and `Drop` still stops the thread
        // and closes the portal session.
        let capture = Self {
            latest,
            stop,
            thread: Some(thread),
            session: Some(session),
            diagnostics,
        };

        match tokio::time::timeout(FIRST_FRAME_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => return Err(err),
            // The sender was dropped without sending: the thread panicked,
            // or returned `Ok` without ever producing a frame.
            Ok(Err(_)) => {
                anyhow::bail!("the screencast thread stopped before producing a frame")
            }
            // The stream is alive and reported no error, but no frame ever
            // landed — without this timeout the await would simply never
            // return.
            Err(_elapsed) => return Err(capture.no_frame_error()),
        }

        let first = capture
            .take_frame()
            .ok_or_else(|| anyhow::anyhow!("screencast signalled a frame but the slot was empty"))?;

        Ok((capture, first))
    }

    /// Builds the error for "waited [`FIRST_FRAME_TIMEOUT`] and got no
    /// frame", naming the most likely cause from what the stream thread
    /// observed — worth doing because the causes are genuinely different
    /// problems (a limitation of this code, the screen being off, a broken
    /// connection) that are otherwise indistinguishable from the outside.
    fn no_frame_error(&self) -> anyhow::Error {
        let d = &self.diagnostics;
        let dequeued = d.buffers_dequeued.load(Ordering::Relaxed);
        let unmapped = d.planes_unmapped.load(Ordering::Relaxed);
        let decoded = d.frames_decoded.load(Ordering::Relaxed);
        let negotiated = d.format_negotiated.load(Ordering::Relaxed);
        let secs = FIRST_FRAME_TIMEOUT.as_secs();

        if unmapped > 0 {
            anyhow::anyhow!(
                "no readable frame after {secs}s: the compositor delivered {unmapped} buffer(s) \
                 with no CPU-mappable data, which means DMA-BUF (GPU memory). colza only reads \
                 shared-memory buffers, so this compositor/driver combination isn't supported \
                 yet — importing DMA-BUF would need an EGL path"
            )
        } else if dequeued == 0 && !negotiated {
            anyhow::anyhow!(
                "no frame after {secs}s and the video format was never negotiated: the PipeWire \
                 stream likely never reached the compositor's node (check that the screen-share \
                 permission wasn't revoked mid-session)"
            )
        } else if dequeued == 0 {
            anyhow::anyhow!(
                "no frame after {secs}s: the video format was negotiated but the compositor sent \
                 no buffers. The captured monitor may be asleep or powered off"
            )
        } else {
            anyhow::anyhow!(
                "no frame after {secs}s: {dequeued} buffer(s) arrived and were mappable but only \
                 {decoded} decoded. Likely an unexpected pixel format, in which case earlier \
                 stderr output names it, or buffers shorter than the negotiated size"
            )
        }
    }

    /// Takes the newest frame, if one has arrived since the last call.
    ///
    /// `None` means nothing new — the caller should keep displaying what
    /// it already has, which is why this is cheap to call every repaint.
    /// The image is moved out of the slot rather than copied, so this
    /// costs a mutex lock and a pointer move regardless of resolution.
    pub fn take_frame(&self) -> Option<RgbImage> {
        self.latest
            .lock()
            .expect("frame slot mutex poisoned by the pipewire thread")
            .take()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // Wakes the mainloop out of `run()` so the thread can unwind. An
        // error here means the receiver is already gone (thread died on
        // its own), in which case the join below returns immediately.
        let _ = self.stop.send(());

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }

        // ashpd's `Session` has no `Drop` of its own, so an unclosed
        // session lingers server-side until the process exits — survivable
        // for the CLI but not for the GUI, which opens a session per pick.
        //
        // Spawned rather than `block_on`: `Drop` runs on the UI thread and
        // must not park it on a D-Bus round-trip, and `block_on` from
        // inside an async context would deadlock outright. The task
        // outliving the process is fine — the portal drops sessions with
        // the client connection anyway; this is just the tidy path.
        if let Some(session) = self.session.take() {
            if let Ok(rt) = crate::runtime::shared() {
                rt.spawn(async move {
                    let _ = session.close().await;
                });
            }
        }
    }
}

/// The one-shot "first frame is in, or setup failed" signal back to
/// [`Capture::open`].
///
/// `Rc<RefCell<Option<..>>>` because either of two places on the PipeWire
/// thread can fire it — the `process` callback on the first frame, or the
/// error path if setup never gets that far — and whichever arrives first
/// takes the sender. `Rc` rather than `Arc` since both live on that one
/// thread.
type ReadySignal =
    std::rc::Rc<std::cell::RefCell<Option<tokio::sync::oneshot::Sender<anyhow::Result<()>>>>>;

/// Drives an entire PipeWire mainloop on the calling (dedicated) thread:
/// connects to the fd the portal handed us, negotiates a raw video format
/// on the given node, and pushes every decoded frame into `latest` until
/// `stop_rx` fires. Blocking, by design.
fn run_stream(
    pw_fd: std::os::fd::OwnedFd,
    node_id: u32,
    latest: Arc<Mutex<Option<RgbImage>>>,
    diagnostics: Arc<StreamDiagnostics>,
    stop_rx: pipewire::channel::Receiver<()>,
    ready: ReadySignal,
) -> anyhow::Result<()> {
    use pipewire::{
        context::ContextRc,
        main_loop::MainLoopRc,
        properties::properties,
        spa::{
            self,
            param::{
                format::{FormatProperties, MediaSubtype, MediaType},
                format_utils,
                video::{VideoFormat, VideoInfoRaw},
                ParamType,
            },
            pod::{serialize::PodSerializer, Pod, Value},
            utils::{Direction, Fraction, Rectangle, SpaTypes},
        },
        stream::{StreamBox, StreamFlags},
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    // Guarded because `open()` may be called many times over a GUI
    // session, each on a fresh thread. `pw_init` is documented as
    // refcounted rather than strictly idempotent, and the matching
    // `pw_deinit` is never called, so calling it once per process is both
    // sufficient and the only shape that stays balanced.
    static PW_INIT: std::sync::Once = std::sync::Once::new();
    PW_INIT.call_once(pipewire::init);

    let mainloop = MainLoopRc::new(None)?;
    let context = ContextRc::new(&mainloop, None)?;
    // `connect_fd_rc` (rather than `connect_rc`, which would talk to the
    // user's default PipeWire socket) is what makes this use the
    // portal-brokered, capture-scoped remote. `_rc` because `StreamBox`
    // needs a `Core` that outlives the borrow, which the plain
    // `connect_fd` does not give.
    let core = context.connect_fd_rc(pw_fd, None)?;

    // Plain `Rc<RefCell<..>>` for the negotiated format: it is written by
    // `param_changed` and read by `process`, both running on this one
    // thread, driven synchronously by `mainloop.run()`. Only `latest`
    // crosses a thread boundary, and that one is an `Arc<Mutex<..>>`.
    let format: Rc<RefCell<VideoInfoRaw>> = Rc::new(RefCell::new(Default::default()));

    let stream = StreamBox::new(
        &core,
        "colza-capture",
        properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let format_for_process = Rc::clone(&format);
    let format_for_param_changed = Rc::clone(&format);
    let diag_for_process = Arc::clone(&diagnostics);
    let diag_for_param_changed = Arc::clone(&diagnostics);

    // `::<()>` because this listener carries no PipeWire-managed user
    // data — the state the callbacks need is captured by the closures
    // instead. Without the turbofish, `D` is unconstrained since neither
    // closure reads its user-data argument.
    let _listener = stream
        .add_local_listener::<()>()
        .param_changed(move |_stream, _user_data, id, param| {
            let Some(param) = param else { return };
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
                return;
            };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }
            match format_for_param_changed.borrow_mut().parse(param) {
                Ok(_) => diag_for_param_changed
                    .format_negotiated
                    .store(true, Ordering::Relaxed),
                Err(err) => eprintln!("colza: failed to parse negotiated video format: {err}"),
            }
        })
        .process(move |stream, _user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            diag_for_process
                .buffers_dequeued
                .fetch_add(1, Ordering::Relaxed);

            let datas = buffer.datas_mut();
            let Some(plane) = datas.first_mut() else {
                return;
            };

            // Read before `plane.data()`: that borrows the plane mutably
            // for as long as the returned slice lives, so `plane.chunk()`
            // is no longer reachable afterwards.
            //
            // `stride` can exceed `width * bytes_per_pixel` (rows padded
            // for alignment), which is why the conversion walks row by row
            // using this rather than assuming a tightly packed buffer.
            let stride = plane.chunk().stride() as usize;

            // `None` here is the DMA-BUF case: the buffer holds a GPU
            // handle rather than mappable memory. Counted rather than just
            // skipped, since it's the one failure mode that means
            // "unsupported" instead of "try again", and `no_frame_error`
            // reports it as such.
            let Some(chunk_data) = plane.data() else {
                diag_for_process
                    .planes_unmapped
                    .fetch_add(1, Ordering::Relaxed);
                return;
            };

            let info = format_for_process.borrow();
            let (width, height) = (info.size().width, info.size().height);
            if width == 0 || height == 0 {
                // Format not negotiated yet — shouldn't happen, since
                // `param_changed` fires before the first `process`, but
                // bail rather than build a zero-sized image if it ever
                // does.
                return;
            }

            let Some(img) = frame_to_rgb(chunk_data, width, height, stride, info.format()) else {
                return;
            };
            diag_for_process
                .frames_decoded
                .fetch_add(1, Ordering::Relaxed);

            // Overwrites whatever the consumer hasn't taken yet: only the
            // newest frame is worth anything, and this is what keeps a
            // slow repaint loop from building a backlog of full-screen
            // images.
            *latest
                .lock()
                .expect("frame slot mutex poisoned by the ui thread") = Some(img);

            // First frame: unblock `open()`. Subsequent frames find `None`
            // here and skip.
            if let Some(tx) = ready.borrow_mut().take() {
                let _ = tx.send(Ok(()));
            }
        })
        .register()?;

    // Only formats with a trivial byte-order mapping to `image::Rgb` are
    // offered — no YUY2/I420, since a YUV->RGB path would be untestable
    // guesswork here and this app only ever reads pixel colors back out.
    // GNOME/mutter normally picks BGRx from this list.
    let obj = spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::RGB,
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: FRAMERATE, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 1000, denom: 1 }
        ),
    );
    let values: Vec<u8> =
        PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
            .map_err(|err| anyhow::anyhow!("failed to serialize pipewire format pod: {err:?}"))?
            .0
            .into_inner();
    let mut params = [Pod::from_bytes(&values)
        .ok_or_else(|| anyhow::anyhow!("failed to build pipewire format Pod from bytes"))?];

    stream.connect(
        Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    // Attached after `connect` but before `run`, and kept in scope for the
    // whole loop — dropping an `AttachedReceiver` detaches it, which would
    // make the stop signal a no-op and hang this thread forever.
    //
    // A weak handle in the callback, because the callback is owned by an
    // io source owned by the loop: capturing a strong `MainLoopRc` would
    // make the loop keep itself alive.
    let weak_loop = mainloop.downgrade();
    let _stop_receiver = stop_rx.attach(mainloop.loop_(), move |()| {
        if let Some(mainloop) = weak_loop.upgrade() {
            mainloop.quit();
        }
    });

    // Blocks this thread, pumping frames into `latest`, until `Capture`'s
    // `Drop` fires the stop channel above (or the compositor tears the
    // stream down, e.g. the user revoked the share).
    mainloop.run();

    Ok(())
}

/// Repacks one PipeWire buffer into a tightly packed `RgbImage`.
///
/// Builds the backing `Vec` directly and hands it to `from_raw` in one
/// pass over the source, rather than a `put_pixel` loop that would pay a
/// bounds check and function call per pixel — invisible for a single
/// snapshot but not for a 30fps full-screen feed.
///
/// Returns `None` for an unsupported format or a short/truncated buffer,
/// which the caller treats as "skip this frame".
fn frame_to_rgb(
    data: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    format: pipewire::spa::param::video::VideoFormat,
) -> Option<RgbImage> {
    use pipewire::spa::param::video::VideoFormat;

    let (w, h) = (width as usize, height as usize);
    let mut out = Vec::with_capacity(w * h * 3);

    // `row` is clipped to the visible width so trailing stride padding is
    // never copied into the image.
    for y in 0..h {
        let start = y * stride;
        match format {
            VideoFormat::RGB => out.extend_from_slice(data.get(start..start + w * 3)?),
            VideoFormat::RGBx | VideoFormat::RGBA => {
                for px in data.get(start..start + w * 4)?.chunks_exact(4) {
                    out.extend_from_slice(&px[..3]);
                }
            }
            VideoFormat::BGRx | VideoFormat::BGRA => {
                for px in data.get(start..start + w * 4)?.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0]]);
                }
            }
            other => {
                eprintln!(
                    "colza: negotiated an unsupported pipewire video format ({other:?}); \
                     this shouldn't happen since only RGB/RGBx/RGBA/BGRx/BGRA were offered"
                );
                return None;
            }
        }
    }

    RgbImage::from_raw(width, height, out)
}

// If the user wants to reset the ScreenCast grant, it lives in the
// compositor's own privacy settings (GNOME: Settings -> Privacy -> Screen
// Sharing), plus `token_store.rs`'s file. Revoking on the compositor side
// while our token file remains just means the next `select_sources` sends
// a stale token, which the portal ignores in favor of prompting — and then
// the new token gets saved. No cleanup needed on our side.