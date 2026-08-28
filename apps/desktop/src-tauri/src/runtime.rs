//! Dictation runtime orchestrator: wires the pure [`Session`] state machine
//! to real adapters (audio capture, STT, refinement, injection, history) on
//! a single dedicated worker thread.
//!
//! ## Design
//!
//! [`Runtime::spawn`] starts one worker thread that owns a `crossbeam`
//! `select!` loop over three channels: hotkey events, typed capture events
//! (audio frames or a terminal stream failure), and
//! control messages ([`RuntimeHandle::cancel`]/`toggle`/`reload`/`shutdown`).
//! Everything the loop needs — the [`Session`], the adapters, and a handful
//! of small pieces of in-flight state (the active capture handle, the
//! silence detector, the current utterance's raw text) — lives entirely on
//! that thread; nothing here is shared across threads except through the
//! channels and the `Arc<dyn EventSink>` the caller supplies. This satisfies
//! the same "Runtime owns everything" shape the design brief sketches,
//! without needing a persistent `Runtime` struct instance: the spawned
//! closure's captured state *is* that ownership.
//!
//! `Session::handle` is pure and total, so this module's job is simply: feed
//! it events, and execute the effects it returns. [`dispatch`] does the
//! former (and emits the resulting phase to the [`EventSink`]); `run_effect`
//! does the latter. Because every step here is synchronous on one thread,
//! executing an effect that itself produces a new event (e.g. finishing
//! transcription, refining, injecting) simply calls back into `dispatch`,
//! forming a straight-line call chain from `HotkeyPressed` down to `Idle`
//! for a single utterance — there is no queue or scheduler to reason about.
//!
//! ## The snippet short-circuit
//!
//! `Session` has no notion of voice snippets — it only knows whether
//! refinement is enabled. So the snippet check happens here, right after
//! `engine.finish()`: dictionary rules are applied to the raw transcript,
//! then [`match_snippet`] is tried on the result. A hit *replaces* the
//! outgoing text with the snippet body and is remembered (in
//! [`PendingUtterance::snippet_hit`]) for the rest of this utterance. If
//! refinement is enabled, `Session` still emits `Effect::Refine` (it doesn't
//! know better) — but when this module executes that effect, a snippet hit
//! makes it feed `Event::RefineDone(body)` straight back into the session
//! without ever calling `ctx.refiner`. This is the one and only bypass of
//! the refiner, and it keeps the "the refiner was never called" guarantee
//! regardless of the user's refine-enabled setting.
//!
//! ## The draft engine, and why it can never leak (spec D9)
//!
//! A profile may carry a second, streaming engine beside the one that
//! produces the text (`ProfileDeps::draft_engine`). The contract is: **while
//! the session is recording, the draft engine is fed every frame the final
//! engine is** — one site, [`handle_audio_frame`], feeding both in turn.
//! Only the draft engine decodes as it goes, and its partials are what the
//! HUD previews while the user is still speaking.
//!
//! The fan-out ends there. The trailing frames drained after capture stops
//! reach the final engine alone; [`stop_capture_and_maybe_transcribe`] says
//! why. Once recording has stopped, the draft engine is normally finished
//! once so a streaming decoder can flush its own trailing context. If an
//! earlier native flush is still stuck, this optional flush is skipped while
//! live preview remains available. An on-time result is collected through a
//! bounded side channel before the authoritative result is committed.
//!
//! Spec D9 requires that this preview can never influence the injected text.
//! That is enforced structurally rather than by care: [`finish_draft`] runs
//! separately, and [`collect_finished_draft`] can send an on-time transcript
//! only to the event sink. Late results restore engine ownership after Idle
//! and discard their text. The transcript that is injected and recorded is
//! still created exclusively by the final engine below.
//!
//! [`handle_partial`] is the single place a partial reaches the UI, which is
//! the seam v0.3 replaces to type into the target application as the user
//! speaks (design spec §11).
//!
//! ## Cancel commit points
//!
//! Because a whole utterance unwinds as one straight-line call chain (see
//! above), a naive implementation would only notice a `Cancel` *after* that
//! chain finished — including after injecting. `Session` explicitly models
//! `CancelRequested` from `Transcribing`/`Refining`/`Injecting` all going to
//! `Idle`, so this module has to actually check for a pending cancel at the
//! two points where the chain would otherwise commit to using a result it
//! blocked on: right after `engine.finish()` returns (before dispatching
//! `TranscriptReady`/`TranscriptFailed`) and right after a refine call
//! resolves — successfully, by failure, or by timeout (before dispatching
//! `RefineDone`/`RefineFailed`, i.e. before the `Inject` effect that
//! transition would produce ever runs). [`check_for_cancel`] does this: a
//! non-blocking drain of the control channel that, if it finds a `Cancel`,
//! makes the caller feed `Event::CancelRequested` instead of the pending
//! event, abandoning the transcript/refine result entirely — nothing is
//! injected. Any *other* control message found during that drain (a
//! `Reload`, a `Toggle`, a `Shutdown`) is not lost: it's queued onto
//! `WorkerCtx::pending_control` and replayed at the top of the main loop
//! once the current utterance has settled to `Idle` or back to `Recording`,
//! preserving arrival order. `engine.finish()` and an in-flight refine call
//! themselves stay blocking/uninterruptible — that's an accepted trade
//! (whisper inference in particular can't be aborted mid-call) — the
//! guarantee this establishes is narrower and sufficient: nothing is
//! *injected* once a cancel has arrived before the corresponding commit
//! point.
//!
//! ## The capture test seam
//!
//! `utter_audio::Capture` touches real audio hardware and is deliberately
//! `!Send` (it must be created, used, and dropped on one thread). Rather
//! than hardcode it, this module depends on the small [`CaptureBackend`] /
//! [`ActiveCapture`] traits; [`RealCaptureBackend`] is the thin production
//! adapter, and tests substitute a scripted fake. `CaptureBackend` itself
//! must be `Send` (it crosses into the worker thread via [`RuntimeDeps`]),
//! but `ActiveCapture` does not: the live capture handle is created *on* the
//! worker thread by a `Send` backend and never leaves it, so `Capture`'s
//! `!Send`-ness is never in tension with this design.
//!
//! A CPAL backend error travels through that same capture channel. While
//! recording it becomes `Event::CaptureFailed`: the pure state machine goes
//! straight to `Idle`, stops the stream, and reports an actionable error,
//! never calling STT `finish()` on the partial utterance. A failure queued
//! after a user-driven stop is ignored because that session already owns its
//! transition to transcription.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{after, select, tick, unbounded, Receiver, Sender};

use utter_audio::{rms_level, AudioError, AudioFrame, CaptureEvent, SilenceDetector};
use utter_core::{
    DictationMode, Effect, Event, InjectionMethod, Session, State, SttEngine, SttError,
    TextInjector, TextRefiner, Tone, TranscribeOptions, Transcript,
};
use utter_inject::{BindingId, HotkeyEvent};
use utter_refine::{apply_rules, match_snippet, ReplaceRule, Snippet};
use utter_store::{HistoryRepo, NewEntry};

use crate::profiles::{ProfileDeps, ProfileRegistry};

/// Idle expiry is intentionally checked much less often than audio/control
/// channels are handled. A model staying resident for at most another 30
/// seconds is harmless; waking the tray worker continuously is not.
const MODEL_EVICTION_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// How much extra time an already-running draft flush may consume after the
/// authoritative engine has finished. A streaming decoder normally returns
/// well inside this window; bounding it keeps a broken preview from holding
/// up injection indefinitely.
const DRAFT_FINISH_GRACE_PERIOD: Duration = Duration::from_millis(100);

/// Sink the runtime reports dictation phase changes and user-facing notices
/// to. `state` matches the `DictationPhase` names in [`crate::events`]
/// (`"idle"`, `"recording"`, `"transcribing"`, `"refining"`, `"injecting"`);
/// `kind` matches `NoticeKind` (`"info"`, `"warning"`, `"error"`).
pub trait EventSink: Send + Sync {
    fn emit_state(&self, state: &str, level: f32, partial: Option<&str>);
    fn notify(&self, kind: &str, msg: &str);
}

/// A handle to the dictation history store. Currently just the concrete
/// [`HistoryRepo`]: `HistoryRepo::add` takes `&self` (rusqlite's
/// `Connection` provides its own interior mutability), so the single-owner
/// worker thread that holds it needs no extra synchronization wrapper.
pub type HistoryHandle = HistoryRepo;

/// A live, in-progress audio capture, created by a [`CaptureBackend`].
///
/// Not `Send`: it is created by the worker thread and only ever used and
/// dropped there (mirroring `utter_audio::Capture`'s own contract), so it
/// never needs to cross a thread boundary.
pub trait ActiveCapture {
    /// Stops capture, flushing any trailing buffered audio to the channel
    /// given to [`CaptureBackend::start`] before returning.
    fn stop(self: Box<Self>);
}

/// Starts microphone capture. The seam between this module and real audio
/// hardware: production code uses [`RealCaptureBackend`], tests substitute a
/// scripted fake that never touches a real device.
pub trait CaptureBackend: Send {
    fn start(
        &self,
        device: Option<&str>,
        tx: Sender<CaptureEvent>,
    ) -> Result<Box<dyn ActiveCapture>, AudioError>;
}

/// Production [`CaptureBackend`]: starts real microphone capture via
/// [`utter_audio::Capture`].
pub struct RealCaptureBackend;

impl CaptureBackend for RealCaptureBackend {
    fn start(
        &self,
        device: Option<&str>,
        tx: Sender<CaptureEvent>,
    ) -> Result<Box<dyn ActiveCapture>, AudioError> {
        utter_audio::Capture::start(device, tx)
            .map(|capture| Box::new(RealActiveCapture(capture)) as Box<dyn ActiveCapture>)
    }
}

struct RealActiveCapture(utter_audio::Capture);

impl ActiveCapture for RealActiveCapture {
    fn stop(self: Box<Self>) {
        self.0.stop();
    }
}

/// Everything [`Runtime::spawn`] needs to drive one dictation session, and
/// everything [`RuntimeHandle::reload`] can swap out for the next one.
///
/// Fields that used to live here directly (`engine`, `refiner`, `refine_enabled`, `tone`,
/// `language`, `engine_label`, `initial_prompt`) are now per-profile instead of per-runtime —
/// see [`profiles`] — because a hotkey binding selects a [`LanguageProfile`], not a single global
/// engine/refiner pair. `profiles` is the only field that changed; everything else here is a
/// true singleton shared by every profile (the hotkey receiver, the history connection, the
/// capture backend, the injector, ...) and could not be duplicated per profile even if it wanted
/// to be.
///
/// [`LanguageProfile`]: utter_store::LanguageProfile
pub struct RuntimeDeps {
    pub mode: DictationMode,
    /// Continuous-silence duration that auto-stops recording; `None`
    /// disables the silence timeout entirely.
    pub silence: Option<Duration>,
    /// Maps each configured profile's hotkey binding to its own engine, refiner, language and
    /// tone, building them lazily on first use. See [`crate::profiles`].
    pub profiles: ProfileRegistry,
    pub injector: Box<dyn TextInjector>,
    /// Whether the selected injection preference promised an automatic
    /// delivery attempt. If every such backend fails and the chain reaches
    /// clipboard-only, the runtime tells the user instead of reporting a
    /// silent success.
    pub automatic_paste_expected: bool,
    pub rules: Vec<ReplaceRule>,
    pub snippets: Vec<Snippet>,
    pub history: Option<HistoryHandle>,
    pub capture_device: Option<String>,
    /// Audio capture backend; [`RealCaptureBackend`] in production.
    pub capture: Box<dyn CaptureBackend>,
    /// The hotkey source's event stream. Owning and re-registering the
    /// actual `HotkeySource` background thread is the app boot path's job
    /// (a later task); this module only ever consumes a `Receiver`, which
    /// keeps it trivially testable — tests drive it with a plain channel.
    pub hotkey_rx: Receiver<HotkeyEvent>,
    pub vad_sensitivity: f32,
    /// How long to wait for a refine call before giving up and using the raw transcript instead.
    /// Shared across every profile: there is one refine endpoint/timeout configured globally
    /// (`RefineCfg`), even though whether it runs and with which tone is per-profile.
    pub refine_timeout: Duration,
}

/// Messages sent from a [`RuntimeHandle`] to the worker thread.
enum ControlMsg {
    Cancel,
    Toggle,
    Reload(Box<RuntimeDeps>),
    Shutdown,
}

/// A running dictation runtime's control handle. Cheap to hold onto; every
/// method besides [`shutdown`](RuntimeHandle::shutdown) just posts a message
/// to the worker thread and returns immediately.
pub struct RuntimeHandle {
    control_tx: Sender<ControlMsg>,
    worker: Option<JoinHandle<()>>,
}

impl RuntimeHandle {
    /// Cancels the in-flight session, if any (no-op when idle). Nothing is
    /// injected for a cancelled session.
    pub fn cancel(&self) {
        let _ = self.control_tx.send(ControlMsg::Cancel);
    }

    /// Drives the session as if the hotkey had just been pressed (from
    /// idle) or as if this mode's "stop" action had just occurred (from
    /// recording); ignored while transcribing/refining/injecting. Lets a UI
    /// affordance (tray menu, HUD button) trigger dictation without a real
    /// hotkey chord.
    pub fn toggle(&self) {
        let _ = self.control_tx.send(ControlMsg::Toggle);
    }

    /// Swaps in new dependencies. If a session is currently recording, it is
    /// cancelled first (nothing injected) rather than queuing the reload —
    /// applying a new engine/refiner/injector mid-utterance would mix state
    /// from two configurations, and settings changes are rare enough that
    /// losing an in-flight recording is an acceptable, clearly-signposted
    /// trade for correctness. Idle sessions swap immediately.
    pub fn reload(&self, deps: RuntimeDeps) {
        let _ = self.control_tx.send(ControlMsg::Reload(Box::new(deps)));
    }

    /// Shuts the worker thread down and waits for it to exit.
    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        let _ = self.control_tx.send(ControlMsg::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RuntimeHandle {
    /// Safety net for callers that drop the handle without calling
    /// `shutdown`: still asks the worker to exit and waits for it, so the
    /// thread never outlives its handle unnoticed. Idempotent with an
    /// explicit `shutdown` call (`worker.take()` guards the double join).
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

/// Namespace for [`Runtime::spawn`]. There is no persistent `Runtime`
/// instance to hold onto — see the module doc comment for why the worker
/// thread's own captured state fills that role instead.
pub struct Runtime;

impl Runtime {
    /// Spawns the worker thread and returns a handle to control it.
    pub fn spawn(deps: RuntimeDeps, sink: Arc<dyn EventSink>) -> RuntimeHandle {
        let (control_tx, control_rx) = unbounded();
        let worker = thread::Builder::new()
            .name("utter-dictation".to_string())
            .spawn(move || worker_loop(deps, sink, control_rx))
            .expect("failed to spawn the utter-dictation worker thread");

        RuntimeHandle {
            control_tx,
            worker: Some(worker),
        }
    }
}

/// The current utterance's raw transcript and whether a voice snippet
/// replaced it, kept around from the moment `engine.finish()` succeeds until
/// injection completes (successfully or not) so `run_refine` can skip the
/// refiner and `record_history` can log both the raw and final text.
struct PendingUtterance {
    raw: String,
    snippet_hit: bool,
}

type DraftFinishResult = (Box<dyn SttEngine>, Result<Transcript, SttError>);

/// Identity and ownership returned by a background draft flush. `generation`
/// prevents an engine taken from an old settings registry from being restored
/// into a replacement registry that happens to reuse the same binding index.
struct DraftFinishOutcome {
    id: u64,
    generation: u64,
    binding: BindingId,
    load_epoch: u64,
    disposition: DraftFinishDisposition,
    result: DraftFinishResult,
}

#[derive(Clone)]
struct DraftFinishTicket {
    id: u64,
    generation: u64,
    load_epoch: u64,
    disposition: DraftFinishDisposition,
}

#[derive(Clone)]
struct DraftFinishDisposition(Arc<AtomicBool>);

impl DraftFinishDisposition {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    fn mark_timed_out(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn timed_out(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Runtime-wide single-worker permit. Dropping it clears the flag during
/// normal return and panic unwinding alike; no `JoinHandle` or blocking join
/// is needed to enforce the concurrency limit.
struct DraftFinishPermit(Arc<AtomicBool>);

impl Drop for DraftFinishPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Everything the worker thread owns for the lifetime of the runtime:
/// swappable adapters/config (from the latest [`RuntimeDeps`]) plus
/// in-flight session state that survives across `select!` iterations.
struct WorkerCtx {
    /// Maps each profile's hotkey binding to its engine/refiner/language/tone. Which entry is
    /// "live" for the in-flight session is tracked separately in `active_binding`, since the
    /// registry itself has no notion of a currently-selected profile.
    profiles: ProfileRegistry,
    /// The binding driving the in-flight session, set by [`start_session_for`] the moment a
    /// press starts one from `State::Idle` (the only moment a binding can be selected — see the
    /// module doc comment) and cleared back to `None` once the session returns to `Idle`. `None`
    /// at rest, between sessions: there is no "current profile" to speak of until a press picks
    /// one.
    active_binding: Option<BindingId>,
    injector: Box<dyn TextInjector>,
    automatic_paste_expected: bool,
    rules: Vec<ReplaceRule>,
    snippets: Vec<Snippet>,
    history: Option<HistoryHandle>,
    capture_device: Option<String>,
    capture: Box<dyn CaptureBackend>,
    hotkey_rx: Receiver<HotkeyEvent>,
    vad_sensitivity: f32,
    silence: Option<Duration>,
    refine_timeout: Duration,
    mode: DictationMode,

    sink: Arc<dyn EventSink>,
    // Kept alive for the runtime's whole lifetime (even with no capture
    // active) so the channel never disconnects: a disconnected receiver
    // would make every `select!` iteration see it as immediately "ready"
    // with an `Err`, spinning the worker thread at 100% CPU forever.
    audio_tx: Sender<CaptureEvent>,
    audio_rx: Receiver<CaptureEvent>,

    active_capture: Option<Box<dyn ActiveCapture>>,
    silence_detector: Option<SilenceDetector>,
    session_started_at: Option<Instant>,
    pending: Option<PendingUtterance>,

    /// The control channel, moved in here (rather than kept as a separate
    /// `worker_loop` local) so the cancel commit points — deep inside the
    /// `dispatch` call chain, not at the top of the loop — can drain it too.
    control_rx: Receiver<ControlMsg>,
    /// Control messages pulled out of `control_rx` by [`check_for_cancel`]
    /// that turned out not to be the `Cancel` it was looking for. Replayed,
    /// in order, at the top of the main loop before the next blocking
    /// `select!` — see the module doc comment ("Cancel commit points").
    pending_control: VecDeque<ControlMsg>,
    /// One native draft `finish()` may run at a time across every profile and
    /// settings generation. A worker clears this through
    /// [`DraftFinishPermit`] even if the decoder panics.
    draft_finish_busy: Arc<AtomicBool>,
    /// Background workers return engine ownership here. The sender remains
    /// runtime-wide so timed-out/cancelled results are not lost and can be
    /// restored later without any thread join.
    draft_outcome_tx: Sender<DraftFinishOutcome>,
    draft_outcome_rx: Receiver<DraftFinishOutcome>,
    /// Incremented on every registry replacement. Outcomes from older
    /// generations are dropped rather than restored into unrelated profiles.
    draft_generation: u64,
    next_draft_finish_id: u64,
    /// A stuck preview is actionable once, not once per utterance or profile.
    /// Late recovery deliberately does not reset this runtime-lifetime latch.
    draft_timeout_notified: bool,
    /// Outcomes received while a session is active. Healthy engines are
    /// restored only after the runtime returns to Idle. Their disposition
    /// decides whether a late error still needs its one user-facing notice.
    deferred_draft_outcomes: VecDeque<DraftFinishOutcome>,
}

impl WorkerCtx {
    fn new(
        deps: RuntimeDeps,
        sink: Arc<dyn EventSink>,
        audio_tx: Sender<CaptureEvent>,
        audio_rx: Receiver<CaptureEvent>,
        control_rx: Receiver<ControlMsg>,
    ) -> Self {
        let (draft_outcome_tx, draft_outcome_rx) = unbounded();
        Self {
            profiles: deps.profiles,
            active_binding: None,
            injector: deps.injector,
            automatic_paste_expected: deps.automatic_paste_expected,
            rules: deps.rules,
            snippets: deps.snippets,
            history: deps.history,
            capture_device: deps.capture_device,
            capture: deps.capture,
            hotkey_rx: deps.hotkey_rx,
            vad_sensitivity: deps.vad_sensitivity,
            silence: deps.silence,
            refine_timeout: deps.refine_timeout,
            mode: deps.mode,
            sink,
            audio_tx,
            audio_rx,
            active_capture: None,
            silence_detector: None,
            session_started_at: None,
            pending: None,
            control_rx,
            pending_control: VecDeque::new(),
            draft_finish_busy: Arc::new(AtomicBool::new(false)),
            draft_outcome_tx,
            draft_outcome_rx,
            draft_generation: 0,
            next_draft_finish_id: 0,
            draft_timeout_notified: false,
            deferred_draft_outcomes: VecDeque::new(),
        }
    }

    /// Swaps in newly-reloaded config/adapters. Runtime-owned in-flight
    /// state (`sink`, audio and draft-outcome channels, capture, and the
    /// runtime-wide draft-worker permit) is left untouched — by the time this
    /// runs the session is idle (see `reload`). The generation advances and
    /// deferred old-registry outcomes are discarded so a late engine can
    /// never be restored into a different settings snapshot.
    /// `active_binding` is likewise already `None` by this point (cleared
    /// the moment the session reached `Idle` — see `dispatch`), but is reset
    /// here too as a defensive invariant: nothing after `apply` should ever
    /// resolve a binding against a registry it no longer belongs to.
    fn apply(&mut self, deps: RuntimeDeps) {
        self.profiles = deps.profiles;
        self.draft_generation = self.draft_generation.wrapping_add(1);
        self.deferred_draft_outcomes.clear();
        self.active_binding = None;
        self.injector = deps.injector;
        self.automatic_paste_expected = deps.automatic_paste_expected;
        self.rules = deps.rules;
        self.snippets = deps.snippets;
        self.history = deps.history;
        self.capture_device = deps.capture_device;
        self.capture = deps.capture;
        self.hotkey_rx = deps.hotkey_rx;
        self.vad_sensitivity = deps.vad_sensitivity;
        self.silence = deps.silence;
        self.refine_timeout = deps.refine_timeout;
        self.mode = deps.mode;
    }
}

/// The [`ProfileDeps`] for the binding driving the in-flight session.
///
/// Panics if called with no active binding, or a binding the registry no longer has an entry
/// for. Both are invariants established by [`start_session_for`] (the only place
/// `ctx.active_binding` is ever set) and preserved afterward: a binding is never removed from a
/// live registry (a `reload` replaces the whole registry, but only once any in-flight recording
/// has already been cancelled back to `Idle` — see `reload` below), and every caller of this
/// function is itself only reachable while a session is in flight (`State::Recording` through
/// `State::Injecting`), which cannot happen before `start_session_for` has run.
fn active_profile(ctx: &mut WorkerCtx) -> &mut ProfileDeps {
    let binding = ctx
        .active_binding
        .expect("invariant: active_profile called with no session in flight");
    ctx.profiles
        .deps_for(binding)
        .map(|(deps, _notices)| deps)
        .expect("invariant: active_binding must resolve to a live registry entry")
}

fn phase_str(state: State) -> &'static str {
    match state {
        State::Idle => "idle",
        State::Recording => "recording",
        State::Transcribing => "transcribing",
        State::Refining => "refining",
        State::Injecting => "injecting",
    }
}

fn worker_loop(deps: RuntimeDeps, sink: Arc<dyn EventSink>, control_rx: Receiver<ControlMsg>) {
    let (audio_tx, audio_rx) = unbounded::<CaptureEvent>();
    let eviction_tick = tick(MODEL_EVICTION_POLL_INTERVAL);
    // Placeholder, replaced by `start_session_for` the moment the first press or `toggle()`
    // selects a profile (see `handle_hotkey_pressed`/`handle_toggle`) and reconstructs this with
    // that profile's own `refine_enabled`. The value here is never actually consulted:
    // `Session::on_idle` doesn't look at `refine_enabled`, and nothing reaches any other state
    // without going through `start_session_for` first — `Session::new` just requires some value
    // to be given one.
    let mut session = Session::new(deps.mode, false);
    let mut ctx = WorkerCtx::new(deps, sink, audio_tx, audio_rx, control_rx);

    loop {
        // Replay anything a cancel-commit-point drain pulled out of
        // `control_rx` and deferred (see `check_for_cancel`) before blocking
        // on `select!` again, so those messages are never lost and stay in
        // arrival order.
        if let Some(msg) = ctx.pending_control.pop_front() {
            if let LoopAction::Exit = handle_control(&mut session, &mut ctx, msg) {
                cleanup_and_exit(&mut ctx);
                return;
            }
            continue;
        }

        select! {
            recv(ctx.hotkey_rx) -> msg => match msg {
                Ok(HotkeyEvent::Pressed { binding }) => {
                    handle_hotkey_pressed(&mut session, &mut ctx, binding)
                }
                // Which binding released is never checked: `ChordMatcher` guarantees only the
                // binding that fired can ever release (see its own doc comment), and the
                // session already knows which one is active via `ctx.active_binding` -- so
                // there is nothing this event needs to add.
                Ok(HotkeyEvent::Released { .. }) => {
                    dispatch(&mut session, &mut ctx, Event::HotkeyReleased)
                }
                // Hotkey source gone (e.g. mid re-registration); nothing to
                // do until a `reload` supplies a fresh receiver.
                Err(_) => {}
            },
            recv(ctx.audio_rx) -> msg => {
                if let Ok(event) = msg {
                    handle_capture_event(&mut session, &mut ctx, event);
                }
            },
            recv(ctx.draft_outcome_rx) -> msg => {
                if let Ok(outcome) = msg {
                    handle_late_draft_outcome(session.state(), &mut ctx, outcome);
                }
            },
            recv(ctx.control_rx) -> msg => match msg {
                Ok(msg) => {
                    if let LoopAction::Exit = handle_control(&mut session, &mut ctx, msg) {
                        cleanup_and_exit(&mut ctx);
                        return;
                    }
                }
                Err(_) => {
                    cleanup_and_exit(&mut ctx);
                    return;
                }
            },
            recv(eviction_tick) -> _ => {
                // The registry also protects `active_binding` itself, but
                // only running the pass at an idle boundary makes the
                // lifetime rule explicit at the orchestrator level too.
                if session.state() == State::Idle {
                    let _ = ctx
                        .profiles
                        .evict_expired(Instant::now(), ctx.active_binding);
                }
            },
        }
    }
}

/// Whether the main loop should keep going after a [`ControlMsg`].
enum LoopAction {
    Continue,
    Exit,
}

fn handle_control(session: &mut Session, ctx: &mut WorkerCtx, msg: ControlMsg) -> LoopAction {
    match msg {
        ControlMsg::Cancel => {
            dispatch(session, ctx, Event::CancelRequested);
            LoopAction::Continue
        }
        ControlMsg::Toggle => {
            handle_toggle(session, ctx);
            LoopAction::Continue
        }
        ControlMsg::Reload(new_deps) => {
            reload(session, ctx, *new_deps);
            LoopAction::Continue
        }
        ControlMsg::Shutdown => LoopAction::Exit,
    }
}

/// Handles a `HotkeyEvent::Pressed { binding }`. A binding is only ever selected from `Idle`
/// (see the module doc comment): if the session is currently idle, this first resolves
/// `binding`'s profile and (re)starts the session with it before dispatching the press;
/// otherwise `binding` is not consulted at all and the press is simply forwarded to the
/// in-flight session as-is (e.g. the second press of a `Toggle`-mode binding, which stops
/// recording regardless of which binding fired it).
fn handle_hotkey_pressed(session: &mut Session, ctx: &mut WorkerCtx, binding: BindingId) {
    if session.state() == State::Idle && !start_session_for(session, ctx, binding) {
        // No profile is registered for this binding -- see `ProfileRegistry::deps_for`'s doc
        // comment for the one thing `None` can mean here. There is nothing sensible to start,
        // so the press is dropped rather than handed to `Session` with no profile behind it.
        return;
    }
    dispatch(session, ctx, Event::HotkeyPressed);
}

/// Resolves `binding` via the registry, surfaces any notice its (possibly first-ever) load
/// produced, records it as the binding driving the new session, and reconstructs `session` fresh
/// with that profile's own `refine_enabled`.
///
/// Reconstructing `Session` here, rather than mutating a long-lived one, is what makes
/// `refine_enabled` a per-press, per-profile value even though `Session::new` only takes it at
/// construction: a session is only ever started from `Idle`, which is also the only moment a
/// binding is selected, so the two happen together and nothing observable is lost — a `Session`
/// carries no other state worth preserving across an idle boundary (`state` is already `Idle`;
/// `mode` does not change per profile).
///
/// Returns `false` (leaving `session`/`ctx` untouched) if `binding` has no registry entry. In
/// practice this is unreachable: `create_source` is only ever handed specs for bindings
/// `ProfileRegistry` also has entries for (see `runtime_boot::parse_profile_hotkeys`, the one
/// place that builds both lists together), so no real `HotkeyEvent` can name an id the registry
/// doesn't know. The check stays because that lockstep is maintained by a *different* module
/// than this one, with nothing at this call site able to verify it holds.
///
/// **Known hazard, accepted and deferred, not fixed here:** `ctx.profiles.deps_for(binding)` can
/// synchronously block this call for as long as a lazy load takes — model I/O (seconds) and, for
/// a profile with refinement configured, `RealProfileLoader` → `build_refiner` →
/// `keyring_password` (`crate::keyring_password`), a `keyring::Entry::get_password()` with no
/// timeout that raises an OS unlock prompt on a locked gnome-keyring/KWallet and blocks until the
/// user answers it. Before profiles, both of these ran once at boot; now they run here, on the
/// worker thread, inside the hotkey-press handler, before `Effect::StartCapture` — while blocked,
/// `worker_loop` is out of its `select!`, so no `"recording"` phase is ever emitted (the user
/// speaks into a stopped microphone) and `cancel`/`toggle`/`reload`/`shutdown` all just queue on
/// `control_rx` (and since `RuntimeHandle::drop` joins the worker, quitting the app hangs behind
/// an unanswered keyring dialog). Bounding this — loading off the worker thread, or a timeout
/// around the keyring call — is its own follow-up; this change inherits the hazard rather than
/// introducing it, since the same calls already blocked boot, but moves it from a one-time,
/// visible startup cost to a per-press, invisible one.
fn start_session_for(session: &mut Session, ctx: &mut WorkerCtx, binding: BindingId) -> bool {
    let Some((deps, notices)) = ctx.profiles.deps_for(binding) else {
        return false;
    };
    let refine_enabled = deps.refine_enabled;

    ctx.active_binding = Some(binding);
    *session = Session::new(ctx.mode, refine_enabled);

    for (kind, msg) in notices {
        ctx.sink.notify(kind, &msg);
    }
    true
}

fn cleanup_and_exit(ctx: &mut WorkerCtx) {
    if let Some(active) = ctx.active_capture.take() {
        active.stop();
    }
}

/// Non-blocking drain of the control channel, used at the two cancel commit
/// points (see the module doc comment). Returns whether a `Cancel` was
/// found; any other message found along the way is preserved in
/// `ctx.pending_control` for the main loop to replay, in order, rather than
/// being silently dropped.
fn check_for_cancel(ctx: &mut WorkerCtx) -> bool {
    let mut cancelled = false;
    while let Ok(msg) = ctx.control_rx.try_recv() {
        if matches!(msg, ControlMsg::Cancel) {
            cancelled = true;
        } else {
            ctx.pending_control.push_back(msg);
        }
    }
    cancelled
}

/// Feeds `event` into the session and executes the resulting transition.
/// `StartCapture` is the one effect run before the phase is emitted: showing
/// the HUD can involve platform UI work, and none of it may delay opening the
/// microphone or cost the first word. Every other effect still runs after
/// the phase emission (in particular, `Inject` only runs after the HUD has
/// been hidden). Effects that complete synchronously and produce a further
/// event call back into `dispatch`, so a whole utterance unwinds as one
/// straight-line call chain.
fn dispatch(session: &mut Session, ctx: &mut WorkerCtx, event: Event) {
    let mut effects = session.handle(event);
    let state_after_transition = session.state();

    if let Some(index) = effects
        .iter()
        .position(|effect| matches!(effect, Effect::StartCapture))
    {
        run_effect(session, ctx, effects.remove(index));
        // A capture failure dispatches its own transition to Idle, including
        // the matching state emission and cleanup. Do not emit that state or
        // clean it up a second time from this superseded outer transition.
        if session.state() != state_after_transition {
            return;
        }
    }

    ctx.sink.emit_state(phase_str(session.state()), 0.0, None);

    for effect in effects {
        run_effect(session, ctx, effect);
    }

    if session.state() == State::Idle {
        ctx.pending = None;
        ctx.session_started_at = None;
        ctx.silence_detector = None;
        // Start the idle timeout after the complete pipeline (including
        // refinement and injection), not after the last audio frame.
        if let Some(binding) = ctx.active_binding {
            ctx.profiles.mark_used(binding, Instant::now());
        }
        // No session is in flight anymore; the next one may pick a different profile (see
        // `start_session_for`), so there is no binding to speak of until it does.
        ctx.active_binding = None;
        restore_deferred_drafts(ctx);
    }
}

fn run_effect(session: &mut Session, ctx: &mut WorkerCtx, effect: Effect) {
    match effect {
        Effect::StartCapture => start_capture(session, ctx),
        Effect::StopCapture => stop_capture_and_maybe_transcribe(session, ctx),
        Effect::Refine(t) => run_refine(session, ctx, t),
        Effect::Inject(text) => run_inject(session, ctx, text),
        Effect::NotifyError(msg) => ctx.sink.notify("error", &msg),
        Effect::NotifyInfo(msg) => ctx.sink.notify("info", &msg),
    }
}

fn start_capture(session: &mut Session, ctx: &mut WorkerCtx) {
    ctx.session_started_at = Some(Instant::now());
    ctx.silence_detector = ctx
        .silence
        .map(|hold| SilenceDetector::new(ctx.vad_sensitivity, hold));

    let (language, initial_prompt) = {
        let deps = active_profile(ctx);
        (deps.language.clone(), deps.initial_prompt.clone())
    };
    let opts = TranscribeOptions {
        language,
        initial_prompt,
    };
    // The draft engine begins on exactly the same options: it is a second
    // view of the same utterance, not a differently-configured one. This is
    // also resets the streaming engine for this utterance; the matching
    // `finish_draft` call at a normal stop flushes and closes that stream.
    //
    // It goes *first*, before the final engine, because the final engine's
    // `begin` can fail and returns below without unwinding the session: a
    // draft engine begun after that point would never be begun at all, while
    // the session it belongs to stays `Recording` and keeps fanning frames
    // out to it. `feed` on an un-begun engine is an invariant violation, and
    // the user would be shown its wording in a toast and lose the preview
    // for the rest of the run (`disable_draft` outlives the session). The
    // two are therefore begun together or not at all; beginning a draft for
    // a session that is about to fail costs nothing, since no frame ever
    // reaches it.
    begin_draft(ctx, &opts);
    // Starting recognition before opening hardware keeps both engines ready
    // for the first frame. A capture-start failure below is then dispatched
    // through `CaptureFailed`, which cancels this begun-but-empty utterance
    // without calling `finish()` or injecting anything.
    if let Err(e) = active_profile(ctx).engine.begin(&opts) {
        ctx.sink
            .notify("error", &format!("failed to start transcription: {e}"));
        return;
    }

    let requested_device = ctx.capture_device.clone();
    let first_attempt = ctx
        .capture
        .start(requested_device.as_deref(), ctx.audio_tx.clone());

    let result = match first_attempt {
        Err(AudioError::DeviceNotFound(name)) if requested_device.is_some() => {
            ctx.sink.notify(
                "warning",
                &format!(
                    "Selected audio input \"{name}\" is unavailable; using the system default \
                     for this run. Your saved device was not changed."
                ),
            );
            // Runtime-only fallback. A settings reload or app restart reads
            // the user's named device again; the persisted preference is
            // never silently rewritten because a Bluetooth/USB device was
            // temporarily absent.
            ctx.capture_device = None;
            ctx.capture.start(None, ctx.audio_tx.clone())
        }
        other => other,
    };

    match result {
        Ok(active) => ctx.active_capture = Some(active),
        Err(error) => dispatch(
            session,
            ctx,
            Event::CaptureFailed(capture_failure_notice(&format!(
                "could not start capture: {error}"
            ))),
        ),
    }
}

fn capture_failure_notice(reason: &str) -> String {
    format!(
        "Audio input stopped: {reason}. Check that the microphone is connected and selected, \
         then press the hotkey to try again."
    )
}

/// Starts the active profile's draft engine, if it has one, on the same
/// options the final engine was just begun with. A failure here degrades
/// exactly like a failing `feed` — see [`disable_draft`] — rather than
/// stopping a session the preview is only an accessory to.
fn begin_draft(ctx: &mut WorkerCtx, opts: &TranscribeOptions) {
    let outcome = active_profile(ctx)
        .draft_engine
        .as_mut()
        .map(|draft| draft.begin(opts));
    if let Some(Err(e)) = outcome {
        disable_draft(ctx, &e.to_string());
    }
}

/// Feeds `samples` to the active profile's draft engine.
///
/// Returns `None` if this profile has no working draft engine (either it
/// never had one, or this very call is what broke it), and `Some(partial)` —
/// where `partial` is itself the engine's `Option` — if it has one. The two
/// levels are deliberately distinct: "no preview to show" and "a preview
/// that happens not to have changed this frame" send the HUD to different
/// places (see [`handle_audio_frame`]).
///
fn feed_draft(ctx: &mut WorkerCtx, samples: &[i16]) -> Option<Option<String>> {
    let outcome = active_profile(ctx)
        .draft_engine
        .as_mut()
        .map(|draft| draft.feed(samples));
    match outcome {
        None => None,
        Some(Ok(partial)) => Some(partial),
        Some(Err(e)) => {
            disable_draft(ctx, &e.to_string());
            None
        }
    }
}

/// Starts finishing the active draft stream on a dedicated thread after a
/// normal recording stop. The caller can therefore start the authoritative
/// engine immediately instead of serialising it behind accessory preview
/// work.
///
/// The atomic permit limits *workers*, not preview engines. If an older
/// native finish is still alive, this session simply skips its optional
/// flush and leaves the current profile's engine in place: live feed keeps
/// working, and its next `begin()` resets the unflushed stream.
fn finish_draft(ctx: &mut WorkerCtx) -> Option<DraftFinishTicket> {
    active_profile(ctx).draft_engine.as_ref()?;
    if ctx
        .draft_finish_busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }

    let binding = ctx
        .active_binding
        .expect("draft finish requires the active session binding");
    let generation = ctx.draft_generation;
    let load_epoch = ctx
        .profiles
        .loaded_epoch(binding)
        .expect("active draft finish requires resident profile dependencies");
    let id = ctx.next_draft_finish_id;
    ctx.next_draft_finish_id = ctx.next_draft_finish_id.wrapping_add(1);
    let disposition = DraftFinishDisposition::new();
    let mut draft = active_profile(ctx)
        .draft_engine
        .take()
        .expect("draft slot checked immediately before taking it");
    let outcome_tx = ctx.draft_outcome_tx.clone();
    let permit = DraftFinishPermit(ctx.draft_finish_busy.clone());
    let outcome_disposition = disposition.clone();
    let worker = thread::Builder::new()
        .name("utter-draft-finish".to_string())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| draft.finish())).unwrap_or_else(|_| {
                Err(SttError::Engine(
                    "preview flush worker panicked".to_string(),
                ))
            });
            // Clear concurrency ownership before publishing the outcome. A
            // new worker may start once this native call is truly over even
            // if the runtime has not processed the late engine yet.
            drop(permit);
            let _ = outcome_tx.send(DraftFinishOutcome {
                id,
                generation,
                binding,
                load_epoch,
                disposition: outcome_disposition,
                result: (draft, result),
            });
        });

    match worker {
        Ok(worker) => {
            // Detached by design. Ownership returns through `draft_outcome_rx`;
            // shutdown never joins a possibly stuck native decoder.
            drop(worker);
            Some(DraftFinishTicket {
                id,
                generation,
                load_epoch,
                disposition,
            })
        }
        Err(e) => {
            // Dropping the failed spawn closure drops `permit` and clears the
            // atomic flag. The taken engine is gone, so this one real failure
            // disables only the active profile's preview.
            disable_draft(ctx, &format!("could not start preview flush: {e}"));
            None
        }
    }
}

/// Restores a late healthy engine only when it still belongs to this registry
/// generation. Its transcript missed the HUD window and is discarded. A late
/// failure after cancellation is still a real model failure and is reported;
/// after a timeout it stays silent because that timeout was already reported.
fn restore_late_draft_outcome(ctx: &mut WorkerCtx, outcome: DraftFinishOutcome) {
    if outcome.generation != ctx.draft_generation
        || ctx.profiles.loaded_epoch(outcome.binding) != Some(outcome.load_epoch)
    {
        return;
    }
    let timed_out = outcome.disposition.timed_out();
    let (draft, result) = outcome.result;
    match result {
        Ok(_) => {
            let _ =
                ctx.profiles
                    .restore_draft_if_loaded(outcome.binding, outcome.load_epoch, draft);
        }
        Err(e) if !timed_out => {
            notify_draft_unavailable(ctx, &e.to_string());
        }
        Err(_) => {
            // The bounded collector already emitted the runtime's one timeout
            // notice. Do not turn the eventual native error into a duplicate.
        }
    }
}

fn handle_late_draft_outcome(state: State, ctx: &mut WorkerCtx, outcome: DraftFinishOutcome) {
    if outcome.generation != ctx.draft_generation {
        return;
    }
    if state == State::Idle {
        restore_late_draft_outcome(ctx, outcome);
    } else {
        ctx.deferred_draft_outcomes.push_back(outcome);
    }
}

/// Pulls all outcomes already queued at an Idle boundary, then restores the
/// healthy engines deferred during Transcribing/Refining/Injecting. This is
/// also the cancellation path's recovery point; no HUD output is possible
/// here.
fn restore_deferred_drafts(ctx: &mut WorkerCtx) {
    while let Ok(outcome) = ctx.draft_outcome_rx.try_recv() {
        if outcome.generation == ctx.draft_generation {
            ctx.deferred_draft_outcomes.push_back(outcome);
        }
    }
    while let Some(outcome) = ctx.deferred_draft_outcomes.pop_front() {
        restore_late_draft_outcome(ctx, outcome);
    }
}

/// Applies the current session's on-time outcome. This is the only place a
/// flushed draft transcript may reach the HUD.
fn apply_current_draft_outcome(ctx: &mut WorkerCtx, outcome: DraftFinishOutcome) {
    if outcome.generation != ctx.draft_generation
        || ctx.profiles.loaded_epoch(outcome.binding) != Some(outcome.load_epoch)
    {
        return;
    }
    let (draft, result) = outcome.result;
    match result {
        Ok(transcript) => {
            let restored =
                ctx.profiles
                    .restore_draft_if_loaded(outcome.binding, outcome.load_epoch, draft);
            let text = transcript.text.trim();
            if restored && !text.is_empty() {
                handle_partial(ctx.sink.as_ref(), "transcribing", 0.0, Some(text));
            }
        }
        Err(e) => notify_draft_unavailable(ctx, &e.to_string()),
    }
}

/// Waits up to the short grace period for draft output while also watching
/// the control channel. Returns `true` when cancellation won. A pending or
/// even already-ready outcome remains on/deferred from the global channel and
/// is restored silently after Idle; cancellation never waits and never emits
/// preview text.
fn collect_finished_draft(ctx: &mut WorkerCtx, ticket: Option<DraftFinishTicket>) -> bool {
    let Some(ticket) = ticket else {
        return false;
    };
    let deadline = Instant::now() + DRAFT_FINISH_GRACE_PERIOD;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            if check_for_cancel(ctx) {
                return true;
            }
            ticket.disposition.mark_timed_out();
            notify_draft_timeout_once(ctx);
            return false;
        }
        let timeout = after(remaining);

        select! {
            recv(ctx.draft_outcome_rx) -> result => {
                if let Ok(outcome) = result {
                    if outcome.id != ticket.id
                        || outcome.generation != ticket.generation
                        || outcome.load_epoch != ticket.load_epoch
                    {
                        handle_late_draft_outcome(State::Transcribing, ctx, outcome);
                        continue;
                    }
                    if check_for_cancel(ctx) {
                        ctx.deferred_draft_outcomes.push_back(outcome);
                        return true;
                    }
                    apply_current_draft_outcome(ctx, outcome);
                    return false;
                }
            },
            recv(ctx.control_rx) -> msg => match msg {
                Ok(ControlMsg::Cancel) => {
                    // The cancellation itself was consumed by this select;
                    // drain any siblings so non-cancel controls retain order.
                    let _ = check_for_cancel(ctx);
                    return true;
                }
                Ok(other) => ctx.pending_control.push_back(other),
                Err(_) => {
                    return true;
                }
            },
            recv(timeout) -> _ => {
                if check_for_cancel(ctx) {
                    return true;
                }
                ticket.disposition.mark_timed_out();
                notify_draft_timeout_once(ctx);
                return false;
            }
        }
    }
}

/// Reports a draft-engine failure once and drops the engine, leaving the
/// profile with no preview and the session otherwise untouched (spec D9: the
/// draft engine's failure must be invisible in the result).
///
/// **This is not "for this session".** `ProfileRegistry` caches a profile's
/// [`ProfileDeps`] for as long as the registry lives (see its doc comment),
/// so clearing `draft_engine` here disables the preview for that profile
/// until the app restarts or settings are reloaded — not just until the
/// hotkey is next pressed. That is the intent rather than an accident: a
/// preview model that failed to decode one frame will fail on the next one
/// too, and a notice per frame — dozens per second — would be far worse
/// than a single one and a dark preview.
///
/// `"info"` and the reassuring tail are not a softening of the message: they
/// are the *same* wording `build_draft_engine` already queues when a preview
/// cannot be built at load time (see `runtime_boot::build_streaming_draft`).
/// The user loses exactly the same thing either way — the preview, never a
/// word of their transcript — and which side of that boundary the failure
/// happened on is an implementation detail they have no way to act on.
fn disable_draft(ctx: &mut WorkerCtx, reason: &str) {
    active_profile(ctx).draft_engine = None;
    notify_draft_unavailable(ctx, reason);
}

fn notify_draft_unavailable(ctx: &WorkerCtx, reason: &str) {
    ctx.sink.notify(
        "info",
        &format!(
            "live preview unavailable: {reason}. Dictation is unaffected — only the live \
             preview is off."
        ),
    );
}

fn notify_draft_timeout_once(ctx: &mut WorkerCtx) {
    if ctx.draft_timeout_notified {
        return;
    }
    ctx.draft_timeout_notified = true;
    notify_draft_unavailable(
        ctx,
        &format!(
            "preview flush exceeded {} ms",
            DRAFT_FINISH_GRACE_PERIOD.as_millis()
        ),
    );
}

/// The single place a recognition partial reaches the UI, and therefore the
/// one function v0.3 changes to type into the target application while the
/// user speaks rather than only painting the HUD (design spec §11).
///
/// [`dispatch`]'s own `emit_state` is not a second such place: it reports a
/// phase change and passes no partial at all, by construction.
fn handle_partial(sink: &dyn EventSink, state: &str, level: f32, partial: Option<&str>) {
    sink.emit_state(state, level, partial);
}

fn handle_audio_frame(session: &mut Session, ctx: &mut WorkerCtx, frame: AudioFrame) {
    if session.state() != State::Recording {
        // Stray frame after capture already stopped (e.g. one last buffered
        // callback firing before the stream handle was dropped); discard.
        return;
    }

    let level = rms_level(&frame.samples);
    let final_partial = match active_profile(ctx).engine.feed(&frame.samples) {
        Ok(partial) => partial,
        Err(e) => {
            ctx.sink
                .notify("warning", &format!("speech engine error: {e}"));
            None
        }
    };
    // The fan-out (design spec §8), and the only one: while recording, the
    // final engine accumulates every frame and the draft engine decodes it
    // now. A profile with a
    // working draft engine shows *its* partial and only its — falling back
    // to the final engine's on a frame the draft merely had nothing new to
    // say about would make the preview flicker between two recognizers.
    // Without one, the final engine's own partial drives the HUD exactly as
    // it did before (which, for every offline engine in the catalog, means
    // no preview at all).
    let partial = feed_draft(ctx, &frame.samples).unwrap_or(final_partial);
    handle_partial(ctx.sink.as_ref(), "recording", level, partial.as_deref());

    let silence_fired = ctx
        .silence_detector
        .as_mut()
        .is_some_and(|detector| detector.observe(level, Instant::now()));
    if silence_fired {
        dispatch(session, ctx, Event::SilenceTimeout);
    }
}

fn handle_capture_event(session: &mut Session, ctx: &mut WorkerCtx, event: CaptureEvent) {
    match event {
        CaptureEvent::Frame(frame) => handle_audio_frame(session, ctx, frame),
        CaptureEvent::StreamFailed(reason) if session.state() == State::Recording => dispatch(
            session,
            ctx,
            Event::CaptureFailed(capture_failure_notice(&reason)),
        ),
        // CPAL may deliver an already-queued terminal callback while a
        // user-driven stop is transcribing. That session already owns its
        // end transition; a late stream error must not finish or notify it a
        // second time.
        CaptureEvent::StreamFailed(_) => {}
    }
}

/// Executes `Effect::StopCapture`: stops the active capture (flushing
/// trailing audio into the channel), drains whatever is now sitting there,
/// and — only if the session actually landed in `Transcribing` (as opposed
/// to `Idle`, e.g. a cancel raced the same effect) — feeds the trailing
/// audio to the engine and runs it to completion.
fn stop_capture_and_maybe_transcribe(session: &mut Session, ctx: &mut WorkerCtx) {
    if let Some(active) = ctx.active_capture.take() {
        active.stop();
    }
    ctx.silence_detector = None;

    let mut trailing = Vec::new();
    while let Ok(event) = ctx.audio_rx.try_recv() {
        if let CaptureEvent::Frame(frame) = event {
            trailing.push(frame);
        }
    }

    if session.state() != State::Transcribing {
        // Cancelled (or superseded by a reload): discard trailing audio,
        // never call finish(), nothing gets injected.
        return;
    }

    for frame in &trailing {
        if let Err(e) = active_profile(ctx).engine.feed(&frame.samples) {
            ctx.sink.notify(
                "warning",
                &format!("speech engine error while flushing: {e}"),
            );
        }
        // Deliberately *not* fanned out to the draft engine: the fan-out
        // ends when recording does (see the module doc comment). This loop
        // runs while the session is already `Transcribing` — the moment the
        // user is waiting for their text — so re-decoding capture-queue
        // leftovers in the accessory model would delay the authoritative
        // result. `finish_draft` below still lets the streaming decoder add
        // and consume its own required trailing context.
    }

    let draft_finish = finish_draft(ctx);
    let result = active_profile(ctx).engine.finish();

    // Commit point 1/2 (see the module doc comment): a `Cancel` that arrived
    // while the authoritative `finish()` was blocking must win immediately,
    // before the optional 100 ms draft grace and before flushed preview text
    // can reach the HUD. The ticket is simply abandoned; the runtime-wide
    // outcome channel restores a healthy engine after Idle without showing
    // its text, and cancellation never waits for accessory work.
    if check_for_cancel(ctx) {
        dispatch(session, ctx, Event::CancelRequested);
        return;
    }

    // A cancel arriving during the bounded collection window wins too. The
    // collector watches `control_rx`, suppresses draft output, and returns
    // without waiting for a native draft call that is still in flight.
    if collect_finished_draft(ctx, draft_finish) {
        dispatch(session, ctx, Event::CancelRequested);
        return;
    }

    match result {
        Ok(t) => {
            let ruled = apply_rules(&t.text, &ctx.rules);
            let (final_text, snippet_hit) = match match_snippet(&ruled, &ctx.snippets) {
                Some(snippet) => (snippet.body.clone(), true),
                None => (ruled, false),
            };
            ctx.pending = Some(PendingUtterance {
                raw: t.text.clone(),
                snippet_hit,
            });
            dispatch(
                session,
                ctx,
                Event::TranscriptReady(Transcript {
                    text: final_text,
                    language: t.language,
                }),
            );
        }
        Err(e) => dispatch(session, ctx, Event::TranscriptFailed(e.to_string())),
    }
}

fn run_refine(session: &mut Session, ctx: &mut WorkerCtx, t: Transcript) {
    let snippet_hit = ctx.pending.as_ref().is_some_and(|p| p.snippet_hit);

    let event = if snippet_hit {
        // The one and only refiner bypass: see the module doc comment.
        Event::RefineDone(t.text)
    } else {
        let (refiner, tone) = {
            let deps = active_profile(ctx);
            (deps.refiner.clone(), deps.tone)
        };
        match refiner {
            Some(refiner) => {
                match refine_with_timeout(refiner, t.text.clone(), tone, ctx.refine_timeout) {
                    Ok(text) => Event::RefineDone(text),
                    Err(reason) => Event::RefineFailed {
                        raw: t.text,
                        reason,
                    },
                }
            }
            None => Event::RefineFailed {
                raw: t.text,
                reason: "no refiner configured".to_string(),
            },
        }
    };

    // Commit point 2/2 (see the module doc comment): a `Cancel` queued
    // while the refine call (or, for a snippet hit, the essentially
    // instantaneous synchronous bypass above) was in flight must win over
    // injecting `event`'s text — abandon it entirely rather than
    // dispatching it, so the `Inject` effect it would produce never runs.
    if check_for_cancel(ctx) {
        dispatch(session, ctx, Event::CancelRequested);
        return;
    }

    dispatch(session, ctx, event);
}

/// Runs `refiner.refine` on a detached thread and races it against
/// `timeout`. A plain (non-scoped) thread is deliberate: `std::thread::scope`
/// would block this call until the spawned thread actually finishes, which
/// defeats the purpose of a timeout if the refiner call itself hangs far
/// longer than `timeout`. A detached thread lets the worker move on the
/// instant the timeout elapses; the abandoned call finishes in the
/// background and its result (sent into a channel nobody is receiving from
/// anymore) is silently dropped.
///
/// Caveat: this only bounds how long *this call* waits, not how long the
/// spawned thread lives. It relies on `refiner.refine` itself eventually
/// returning (e.g. via its own HTTP client timeout) — a `TextRefiner` impl
/// with no internal timeout of its own, racing against a network call that
/// simply hangs forever, would leak one thread per such call.
fn refine_with_timeout(
    refiner: Arc<dyn TextRefiner>,
    text: String,
    tone: Tone,
    timeout: Duration,
) -> Result<String, String> {
    let (tx, rx) = crossbeam_channel::bounded(1);

    thread::spawn(move || {
        let result = refiner.refine(&text, tone).map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    rx.recv_timeout(timeout)
        .unwrap_or_else(|_| Err("refine request timed out".to_string()))
}

fn run_inject(session: &mut Session, ctx: &mut WorkerCtx, text: String) {
    match ctx.injector.inject(&text) {
        Ok(method) => {
            if method == InjectionMethod::ClipboardOnly && ctx.automatic_paste_expected {
                ctx.sink.notify(
                    "warning",
                    "Automatic paste was unavailable, so the text was copied to the clipboard. \
                     Check text-injection permission and keep the target field focused.",
                );
            }
            record_history(ctx, &text);
            dispatch(session, ctx, Event::InjectDone(method));
        }
        Err(e) => dispatch(session, ctx, Event::InjectFailed(e.to_string())),
    }
}

fn record_history(ctx: &mut WorkerCtx, final_text: &str) {
    let profile = active_profile(ctx);
    let engine_label = profile.engine_label.clone();
    let profile_id = profile.profile_id.clone();

    let (Some(history), Some(pending)) = (&ctx.history, &ctx.pending) else {
        return;
    };

    let duration_ms = ctx
        .session_started_at
        .map(|started| started.elapsed().as_millis() as i64)
        .unwrap_or(0);

    let entry = NewEntry {
        duration_ms,
        engine: engine_label,
        raw_text: pending.raw.clone(),
        final_text: final_text.to_string(),
        app: None,
        profile_id: Some(profile_id),
    };

    if let Err(e) = history.add(entry) {
        ctx.sink
            .notify("warning", &format!("failed to save history entry: {e}"));
    }
}

/// Drives the session as if a physical hotkey chord had just fired,
/// translating today's mode into the right half of the press/release pair:
/// idle starts recording; recording stops it (a second press for `Toggle`, a
/// release for `PushToTalk`); any busier state is ignored; there is nothing
/// sensible for a single button to do mid-pipeline.
fn handle_toggle(session: &mut Session, ctx: &mut WorkerCtx) {
    match session.state() {
        State::Idle => {
            // A UI-triggered toggle (tray menu, HUD button) carries no hotkey binding of its
            // own, so it always starts the default profile's session -- binding 0, the same
            // profile `ProfileRegistry::new` eagerly loads at boot (see its doc comment).
            if start_session_for(session, ctx, BindingId::from(0)) {
                dispatch(session, ctx, Event::HotkeyPressed);
            } else {
                // No profile at all (e.g. every profile was dropped for an unparseable hotkey,
                // or `profiles = []`): `parse_profile_hotkeys`/`ProfileRegistry::new` already
                // warned about this once at boot, but that toast is dismissible and this is the
                // affordance the user reaches for afterwards -- it must say something too,
                // rather than the tray item/HUD button silently doing nothing.
                ctx.sink.notify(
                    "warning",
                    "no language profile is configured; dictation has no hotkey until at \
                     least one profile is configured",
                );
            }
        }
        State::Recording => {
            let event = match ctx.mode {
                DictationMode::Toggle => Event::HotkeyPressed,
                DictationMode::PushToTalk => Event::HotkeyReleased,
            };
            dispatch(session, ctx, event);
        }
        State::Transcribing | State::Refining | State::Injecting => {}
    }
}

/// Applies reloaded dependencies. If a session is recording, it is
/// cancelled first (see [`RuntimeHandle::reload`]'s doc comment for why);
/// by the time `ctx.apply` runs, the session is always idle.
///
/// `session` is reset to a placeholder here, the same way `worker_loop` seeds its very first
/// one: `refine_enabled` is per-profile now, so there is no single value to give `Session::new`
/// until the next press picks a binding and `start_session_for` reconstructs it for real (see
/// that function's doc comment).
fn reload(session: &mut Session, ctx: &mut WorkerCtx, new_deps: RuntimeDeps) {
    if session.state() == State::Recording {
        dispatch(session, ctx, Event::CancelRequested);
    }
    *session = Session::new(new_deps.mode, false);
    ctx.apply(new_deps);
}
