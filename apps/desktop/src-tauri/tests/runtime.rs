//! Integration tests for the dictation runtime orchestrator: drives
//! `Runtime::spawn` through scripted fakes for every adapter (STT engine,
//! refiner, injector, capture backend) and a real, temp-dir-backed
//! `HistoryRepo`, asserting on the observable state sequence, notices, and
//! injected/recorded text — never on internal implementation details.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};

use utter_audio::{AudioError, AudioFrame, CaptureEvent};
use utter_core::{
    DictationMode, InjectError, InjectionMethod, RefineError, SttEngine, SttError, TextInjector,
    TextRefiner, Tone, TranscribeOptions, Transcript,
};
use utter_desktop_lib::profiles::{ProfileDeps, ProfileLoader, ProfileRegistry};
use utter_desktop_lib::runtime::{ActiveCapture, CaptureBackend, EventSink, Runtime, RuntimeDeps};
use utter_inject::{BindingId, HotkeyEvent};
use utter_refine::{ReplaceRule, Snippet};
use utter_store::{HistoryRepo, LanguageProfile};

/// Generous but bounded: every wait in these tests uses this instead of an
/// unbounded `recv`, so a regression that stalls the worker fails the test
/// promptly rather than hanging the suite.
const WAIT: Duration = Duration::from_secs(5);

// ---- fakes ------------------------------------------------------------

/// One call the fake STT engine recorded, in the order it happened. Lets
/// tests assert ordering (e.g. "every `Feed` precedes the `Finish`") rather
/// than just call counts.
#[derive(Debug, Clone, PartialEq)]
enum CallRecord {
    Feed(Vec<i16>),
    Finish,
}

/// Speech-to-text engine that returns a fixed, scripted result from
/// `finish()` regardless of what was fed to it, records every `feed`/
/// `finish` call (in order, with `feed`'s samples) into a shared log, and
/// can optionally return a scripted partial from `feed` or block for a bit
/// inside `finish` (to widen a real-time window for a racing `cancel()`).
struct FakeSttEngine {
    result: Result<Transcript, SttError>,
    calls: Arc<Mutex<Vec<CallRecord>>>,
    partial: Option<String>,
    finish_delay: Duration,
    begin_opts: Arc<Mutex<Vec<TranscribeOptions>>>,
}

impl SttEngine for FakeSttEngine {
    fn begin(&mut self, opts: &TranscribeOptions) -> Result<(), SttError> {
        self.begin_opts.lock().expect("lock").push(opts.clone());
        Ok(())
    }

    fn feed(&mut self, samples: &[i16]) -> Result<Option<String>, SttError> {
        self.calls
            .lock()
            .expect("lock")
            .push(CallRecord::Feed(samples.to_vec()));
        Ok(self.partial.clone())
    }

    fn finish(&mut self) -> Result<Transcript, SttError> {
        if !self.finish_delay.is_zero() {
            thread::sleep(self.finish_delay);
        }
        self.calls.lock().expect("lock").push(CallRecord::Finish);
        self.result.clone()
    }
}

/// An engine that always fails, either from `begin` or from every `feed`, recording every call it
/// was asked to make. Used as a *draft* engine, where the runtime's policy is to warn once, drop
/// it, and carry on: the recorded calls are what prove the drop actually happened (a runtime that
/// merely swallowed the error would keep calling it on every subsequent frame).
struct FailingSttEngine {
    fail_begin: bool,
    calls: Arc<Mutex<Vec<CallRecord>>>,
}

impl SttEngine for FailingSttEngine {
    fn begin(&mut self, _opts: &TranscribeOptions) -> Result<(), SttError> {
        if self.fail_begin {
            return Err(SttError::Engine("draft model failed to start".to_string()));
        }
        Ok(())
    }

    fn feed(&mut self, samples: &[i16]) -> Result<Option<String>, SttError> {
        self.calls
            .lock()
            .expect("lock")
            .push(CallRecord::Feed(samples.to_vec()));
        Err(SttError::Engine("draft model decode failed".to_string()))
    }

    fn finish(&mut self) -> Result<Transcript, SttError> {
        self.calls.lock().expect("lock").push(CallRecord::Finish);
        Err(SttError::Engine("draft model decode failed".to_string()))
    }
}

/// An STT engine whose `finish()` cannot return until the test opens a gate.
/// Used for both final and draft roles wherever concurrency must be observable
/// without relying on relative sleep durations.
struct GatedFinishSttEngine {
    calls: Arc<Mutex<Vec<CallRecord>>>,
    started_tx: Sender<()>,
    release_rx: Receiver<()>,
    returned_tx: Sender<()>,
}

impl SttEngine for GatedFinishSttEngine {
    fn begin(&mut self, _opts: &TranscribeOptions) -> Result<(), SttError> {
        Ok(())
    }

    fn feed(&mut self, samples: &[i16]) -> Result<Option<String>, SttError> {
        self.calls
            .lock()
            .expect("lock")
            .push(CallRecord::Feed(samples.to_vec()));
        Ok(Some("working preview".to_string()))
    }

    fn finish(&mut self) -> Result<Transcript, SttError> {
        self.calls.lock().expect("lock").push(CallRecord::Finish);
        let _ = self.started_tx.send(());
        let _ = self.release_rx.recv();
        let _ = self.returned_tx.send(());
        Ok(transcript("late draft result"))
    }
}

/// A gated authoritative engine whose live `feed` intentionally has no text,
/// so a recovery probe can attribute `working preview` only to the restored
/// draft engine rather than to the runtime's final-engine fallback.
struct SilentGatedFinishSttEngine(GatedFinishSttEngine);

impl SttEngine for SilentGatedFinishSttEngine {
    fn begin(&mut self, opts: &TranscribeOptions) -> Result<(), SttError> {
        self.0.begin(opts)
    }

    fn feed(&mut self, samples: &[i16]) -> Result<Option<String>, SttError> {
        self.0.feed(samples).map(|_| None)
    }

    fn finish(&mut self) -> Result<Transcript, SttError> {
        self.0.finish()
    }
}

/// The failure twin of [`GatedFinishSttEngine`]. Keeping the error behind a
/// gate lets cancellation/timeout win first, so tests exercise the *late*
/// outcome policy rather than the ordinary on-time failure path.
struct GatedFailingFinishSttEngine {
    calls: Arc<Mutex<Vec<CallRecord>>>,
    started_tx: Sender<()>,
    release_rx: Receiver<()>,
    returned_tx: Sender<()>,
    dropped_tx: Option<Sender<()>>,
}

impl SttEngine for GatedFailingFinishSttEngine {
    fn begin(&mut self, _opts: &TranscribeOptions) -> Result<(), SttError> {
        Ok(())
    }

    fn feed(&mut self, samples: &[i16]) -> Result<Option<String>, SttError> {
        self.calls
            .lock()
            .expect("lock")
            .push(CallRecord::Feed(samples.to_vec()));
        Ok(Some("working preview".to_string()))
    }

    fn finish(&mut self) -> Result<Transcript, SttError> {
        self.calls.lock().expect("lock").push(CallRecord::Finish);
        let _ = self.started_tx.send(());
        let _ = self.release_rx.recv();
        let _ = self.returned_tx.send(());
        Err(SttError::Engine("late draft flush failed".to_string()))
    }
}

impl Drop for GatedFailingFinishSttEngine {
    fn drop(&mut self) {
        if let Some(dropped_tx) = &self.dropped_tx {
            let _ = dropped_tx.send(());
        }
    }
}

/// Wraps a gated engine and signals when runtime ownership is finally
/// discarded. In the reload regression that happens only after the old
/// generation's late outcome has crossed the global outcome channel, which
/// also proves its atomic worker permit was released before the next session.
struct DropNotifyingSttEngine {
    inner: GatedFinishSttEngine,
    dropped_tx: Sender<()>,
}

impl SttEngine for DropNotifyingSttEngine {
    fn begin(&mut self, opts: &TranscribeOptions) -> Result<(), SttError> {
        self.inner.begin(opts)
    }

    fn feed(&mut self, samples: &[i16]) -> Result<Option<String>, SttError> {
        self.inner.feed(samples)
    }

    fn finish(&mut self) -> Result<Transcript, SttError> {
        self.inner.finish()
    }
}

impl Drop for DropNotifyingSttEngine {
    fn drop(&mut self) {
        let _ = self.dropped_tx.send(());
    }
}

/// A working draft engine: emits `partial` from every `feed`, records every call into `calls`,
/// and records the options it was begun with into `begin_opts`.
///
/// `begin_opts` is caller-supplied rather than a private `Arc` this function makes up, because
/// options nobody holds a handle to cannot be asserted on: the runtime must begin the draft
/// engine on the *same* `TranscribeOptions` as the final one, and a fixture that swallowed them
/// would leave that unfalsifiable (see
/// `the_draft_engine_begins_on_the_same_options_as_the_final_engine`).
///
/// Its `finish()` transcript is deliberately unmistakable: the runtime may show it as the final
/// HUD preview, but must never inject it or put it in history (spec D9).
fn draft_engine(
    partial: &str,
    calls: Arc<Mutex<Vec<CallRecord>>>,
    begin_opts: Arc<Mutex<Vec<TranscribeOptions>>>,
) -> Box<dyn SttEngine> {
    Box::new(FakeSttEngine {
        result: Ok(transcript("DRAFT-FINAL-PREVIEW")),
        calls,
        partial: Some(partial.to_string()),
        finish_delay: Duration::ZERO,
        begin_opts,
    })
}

/// The `Debug` rendering of every `TranscribeOptions` an engine was begun with.
///
/// `TranscribeOptions` implements `Debug` but not `PartialEq`, and `utter-core` is deliberately
/// not modified for a test's convenience, so two engines' options are compared by rendering.
/// That has a property field-by-field assertions lack: a field added to `TranscribeOptions`
/// later is covered automatically instead of silently escaping the comparison.
fn begun_with(opts: &Arc<Mutex<Vec<TranscribeOptions>>>) -> Vec<String> {
    opts.lock()
        .expect("lock")
        .iter()
        .map(|opts| format!("{opts:?}"))
        .collect()
}

/// The samples of every `feed` in a recorded call log, in order — the shape the frame fan-out
/// invariant is stated in ("both engines saw the same frames"), with `Finish` filtered out since
/// only one of the two engines is ever finished.
fn fed_samples(calls: &Arc<Mutex<Vec<CallRecord>>>) -> Vec<Vec<i16>> {
    calls
        .lock()
        .expect("lock")
        .iter()
        .filter_map(|call| match call {
            CallRecord::Feed(samples) => Some(samples.clone()),
            CallRecord::Finish => None,
        })
        .collect()
}

/// Waits until `calls` has recorded `count` `feed`s.
///
/// The fan-out contract is scoped to *recording*, so a test that pushes frames and then
/// releases the hotkey has to know its frames actually took the live recording path rather than
/// being left for `StopCapture`'s trailing drain -- which the `select!` loop, not the test,
/// decides. Waiting on the call log itself settles that by construction: nothing else advances
/// it. Bounded by `WAIT` so a runtime that stops feeding fails the test loudly instead of
/// hanging the suite.
fn wait_for_feeds(calls: &Arc<Mutex<Vec<CallRecord>>>, count: usize) {
    let deadline = Instant::now() + WAIT;
    loop {
        let fed = fed_samples(calls);
        if fed.len() >= count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {count} frames to have been fed within {WAIT:?}, got {fed:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn transcript(text: &str) -> Transcript {
    Transcript {
        text: text.to_string(),
        language: None,
    }
}

/// Refiner whose behavior (uppercase / fail / succeed-after-a-delay) is
/// scripted, with a shared call counter tests assert on to prove (or
/// disprove) it ran. `Delay` is used to widen a real-time window for a
/// racing `cancel()` in the cancel-during-refine test.
enum RefineBehavior {
    Uppercase,
    Fail(String),
    Delay(Duration),
}

struct FakeRefiner {
    behavior: RefineBehavior,
    calls: Arc<AtomicUsize>,
}

impl TextRefiner for FakeRefiner {
    fn refine(&self, text: &str, _tone: Tone) -> Result<String, RefineError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.behavior {
            RefineBehavior::Uppercase => Ok(text.to_uppercase()),
            RefineBehavior::Fail(msg) => Err(RefineError::Http(msg.clone())),
            RefineBehavior::Delay(d) => {
                thread::sleep(*d);
                Ok(text.to_uppercase())
            }
        }
    }
}

/// Records every string handed to `inject`, optionally failing instead.
struct FakeInjector {
    injected: Arc<Mutex<Vec<String>>>,
    fail: bool,
    method: InjectionMethod,
}

impl TextInjector for FakeInjector {
    fn inject(&mut self, text: &str) -> Result<InjectionMethod, InjectError> {
        if self.fail {
            return Err(InjectError::Backend("injection failed".to_string()));
        }
        self.injected.lock().expect("lock").push(text.to_string());
        Ok(self.method)
    }
}

/// Never touches real audio hardware. Hands back a no-op capture handle,
/// and — this is the point of it — stashes the `Sender<CaptureEvent>` it was
/// given into a shared slot, so a test can fetch it after "recording" starts
/// and push scripted `AudioFrame`s through the exact same channel the real
/// worker loop reads from.
struct FakeCaptureBackend {
    tx_slot: Arc<Mutex<Option<Sender<CaptureEvent>>>>,
}

impl CaptureBackend for FakeCaptureBackend {
    fn start(
        &self,
        _device: Option<&str>,
        tx: Sender<CaptureEvent>,
    ) -> Result<Box<dyn ActiveCapture>, AudioError> {
        *self.tx_slot.lock().expect("lock") = Some(tx);
        Ok(Box::new(NoopActiveCapture))
    }
}

/// Records every requested device and reports a named device as missing.
/// The default-device retry can either succeed or return a scripted error.
struct MissingSelectedCaptureBackend {
    calls: Arc<Mutex<Vec<Option<String>>>>,
    tx_slot: Arc<Mutex<Option<Sender<CaptureEvent>>>>,
    default_error: Option<AudioError>,
}

impl CaptureBackend for MissingSelectedCaptureBackend {
    fn start(
        &self,
        device: Option<&str>,
        tx: Sender<CaptureEvent>,
    ) -> Result<Box<dyn ActiveCapture>, AudioError> {
        self.calls
            .lock()
            .expect("lock")
            .push(device.map(str::to_string));

        if let Some(name) = device {
            return Err(AudioError::DeviceNotFound(name.to_string()));
        }
        if let Some(error) = &self.default_error {
            return Err(error.clone());
        }

        *self.tx_slot.lock().expect("lock") = Some(tx);
        Ok(Box::new(NoopActiveCapture))
    }
}

/// A successful backend whose start count proves a stream failure does not
/// poison the factory for the next hotkey press.
struct CountingCaptureBackend {
    starts: Arc<AtomicUsize>,
    tx_slot: Arc<Mutex<Option<Sender<CaptureEvent>>>>,
}

/// Marks the instant capture is opened, so a sink can prove the initial
/// `recording` phase never runs platform UI work ahead of the microphone.
struct CaptureOrderBackend {
    started: Arc<AtomicBool>,
}

impl CaptureBackend for CaptureOrderBackend {
    fn start(
        &self,
        _device: Option<&str>,
        _tx: Sender<CaptureEvent>,
    ) -> Result<Box<dyn ActiveCapture>, AudioError> {
        self.started.store(true, Ordering::SeqCst);
        Ok(Box::new(NoopActiveCapture))
    }
}

impl CaptureBackend for CountingCaptureBackend {
    fn start(
        &self,
        _device: Option<&str>,
        tx: Sender<CaptureEvent>,
    ) -> Result<Box<dyn ActiveCapture>, AudioError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        *self.tx_slot.lock().expect("lock") = Some(tx);
        Ok(Box::new(NoopActiveCapture))
    }
}

struct NoopActiveCapture;

impl ActiveCapture for NoopActiveCapture {
    fn stop(self: Box<Self>) {}
}

/// One `emit_state` call: phase, level, and partial, in the shape tests need
/// to check all three (most tests only care about the phase; a couple check
/// the partial too).
type Emission = (String, f32, Option<String>);

/// One `notify` call: kind ("info"/"error") and message.
type Notice = (String, String);

/// Records every `emit_state` call and every `notify` call, each pushed to
/// its own channel so tests can wait on the *next* one with a bounded
/// timeout instead of polling or inferring it from an unrelated event. The
/// two channels are independent: nothing here orders an emission against a
/// notice beyond what `dispatch`/`run_effect` themselves guarantee, so a
/// test that wants a notice must wait for it explicitly via `recv_notice`
/// rather than assuming it already happened because some state was observed.
struct FakeSink {
    states_tx: Sender<Emission>,
    notices_tx: Sender<Notice>,
}

struct CaptureOrderSink {
    capture_started: Arc<AtomicBool>,
    recording_tx: Sender<bool>,
}

impl EventSink for CaptureOrderSink {
    fn emit_state(&self, state: &str, _level: f32, _partial: Option<&str>) {
        if state == "recording" {
            let _ = self
                .recording_tx
                .send(self.capture_started.load(Ordering::SeqCst));
        }
    }

    fn notify(&self, _kind: &str, _msg: &str) {}
}

impl EventSink for FakeSink {
    fn emit_state(&self, state: &str, level: f32, partial: Option<&str>) {
        let _ = self
            .states_tx
            .send((state.to_string(), level, partial.map(str::to_string)));
    }

    fn notify(&self, kind: &str, msg: &str) {
        let _ = self.notices_tx.send((kind.to_string(), msg.to_string()));
    }
}

/// Waits for the next emission and returns just its phase — what almost
/// every test wants.
fn recv_state(rx: &Receiver<Emission>) -> String {
    rx.recv_timeout(WAIT)
        .expect("expected a dictation-state emission within the timeout")
        .0
}

/// Waits for emissions, skipping any whose phase doesn't match `expected` —
/// needed once a test pushes audio frames, since each processed frame emits
/// its own `"recording"` (possibly repeated several times) before the next
/// real transition.
fn recv_until(rx: &Receiver<Emission>, expected: &str) {
    loop {
        let (state, _, _) = rx
            .recv_timeout(WAIT)
            .expect("expected a dictation-state emission within the timeout");
        if state == expected {
            return;
        }
    }
}

/// Waits for the next emission that carries a partial transcript, ignoring
/// any (e.g. the initial `"recording"` from `StartCapture`) that don't.
fn recv_partial_emission(rx: &Receiver<Emission>) -> Emission {
    loop {
        let emission = rx
            .recv_timeout(WAIT)
            .expect("expected a dictation-state emission within the timeout");
        if emission.2.is_some() {
            return emission;
        }
    }
}

fn recv_partial(rx: &Receiver<Emission>) -> Option<String> {
    recv_partial_emission(rx).2
}

/// Bounded probe used by late-restoration regressions. If a press races the
/// worker's outcome-channel receive, that session has no draft engine yet;
/// the test cancels it and retries after the resulting Idle boundary drains
/// the queued outcome.
fn recv_partial_before(rx: &Receiver<Emission>, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match rx.recv_timeout(remaining) {
            Ok((_, _, Some(partial))) => return Some(partial),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Waits for the next notice with the same bounded timeout `recv_state`
/// uses, instead of inferring a notice happened from some unrelated state
/// emission — `emit_state` and `notify` are ordered relative to each other
/// only by whatever `dispatch`/`run_effect` actually guarantee (see
/// `run_effect`'s effect ordering), which for some transitions puts the
/// notice *after* a state a test has already observed.
fn recv_notice(rx: &Receiver<Notice>) -> Notice {
    rx.recv_timeout(WAIT)
        .expect("expected a notify() call within the timeout")
}

/// Asserts the one notice a draft engine's death produces, whichever end of its life it died at.
///
/// Both the severity and the tail are pinned, and deliberately together: the loss is `"info"`,
/// not the `"warning"` a lost *transcript* earns, and it carries the same closing sentence
/// `runtime_boot::build_streaming_draft` appends when a preview cannot be built in the first
/// place. Asserting the kind alone would let the two halves of the preview's life drift back
/// into describing the identical loss in two different voices, which is what this reunites.
fn assert_preview_lost_notice(rx: &Receiver<Notice>) {
    let (kind, msg) = recv_notice(rx);
    assert_eq!(
        kind, "info",
        "a lost preview costs no transcript and reports as info, like every loader-side preview \
         failure; got kind {kind:?} with {msg:?}"
    );
    assert!(
        msg.contains("live preview unavailable"),
        "expected a preview-unavailable notice, got {msg:?}"
    );
    assert!(
        msg.contains("Dictation is unaffected — only the live preview is off."),
        "expected the same tail the loader-side preview failures carry, got {msg:?}"
    );
}

fn assert_no_more_states(rx: &Receiver<Emission>) {
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "expected no further state emissions"
    );
}

fn assert_no_more_notices(rx: &Receiver<Notice>) {
    let extra = rx.recv_timeout(Duration::from_millis(200));
    assert!(
        extra.is_err(),
        "expected no further notices, got {:?}",
        extra.ok()
    );
}

/// A `ProfileLoader` that hands back a pre-built `ProfileDeps` for each profile id exactly once
/// (`.remove()`s it out of a per-id slot), panicking if the registry ever asks for the same
/// profile's deps a second time -- which only happens if a test's own profile list has a
/// duplicate id, a fixture bug this is deliberately strict about rather than silently rebuilding
/// or reusing something. `ProfileRegistry` itself only ever calls `load` once per entry and
/// caches the result forever (see its own doc comment), so this is never exercised twice for the
/// same id in practice.
struct FakeProfileLoader {
    slots: Mutex<HashMap<String, ProfileDeps>>,
}

impl ProfileLoader for FakeProfileLoader {
    fn load(&self, profile: &LanguageProfile) -> (ProfileDeps, Vec<(&'static str, String)>) {
        let deps = self
            .slots
            .lock()
            .expect("lock")
            .remove(&profile.id)
            .unwrap_or_else(|| panic!("no fixture registered for profile \"{}\"", profile.id));
        (deps, Vec::new())
    }
}

/// Builds a `ProfileRegistry` over `profiles`, each paired with the exact `ProfileDeps` it must
/// resolve to -- the hotkey string in each `LanguageProfile` is never consulted by the worker
/// (only `runtime_boot::parse_profile_hotkeys` reads it, an earlier step these tests bypass by
/// constructing `RuntimeDeps` directly and driving `hotkey_rx` with synthetic `BindingId`s), so
/// any placeholder works.
fn registry_with(profiles_and_deps: Vec<(LanguageProfile, ProfileDeps)>) -> ProfileRegistry {
    let mut slots = HashMap::new();
    let mut profiles = Vec::new();
    for (profile, deps) in profiles_and_deps {
        slots.insert(profile.id.clone(), deps);
        profiles.push(profile);
    }
    let loader = Box::new(FakeProfileLoader {
        slots: Mutex::new(slots),
    });
    let (registry, _notices) = ProfileRegistry::new(profiles, loader);
    registry
}

fn test_profile(id: &str) -> LanguageProfile {
    LanguageProfile {
        id: id.to_string(),
        ..LanguageProfile::default()
    }
}

/// Common `RuntimeDeps`/`ProfileDeps` fields every test wants; individual fields are overridden
/// per test before calling `build`. Always builds a `ProfileRegistry` with exactly one profile
/// (id `"default"`, binding 0) -- every existing test presses/toggles `BindingId::from(0)`, so a
/// single-profile registry keeps them all exercising the same worker-side behaviour they always
/// have. Multi-profile routing itself is covered separately (see
/// `each_hotkey_dictates_with_its_own_profile`).
struct DepsBuilder {
    mode: DictationMode,
    refine_enabled: bool,
    engine_result: Result<Transcript, SttError>,
    calls: Arc<Mutex<Vec<CallRecord>>>,
    partial: Option<String>,
    finish_delay: Duration,
    refiner: Option<(RefineBehavior, Arc<AtomicUsize>)>,
    inject_fail: bool,
    injection_method: InjectionMethod,
    automatic_paste_expected: bool,
    injected: Arc<Mutex<Vec<String>>>,
    rules: Vec<ReplaceRule>,
    snippets: Vec<Snippet>,
    history: Option<HistoryRepo>,
    silence: Option<Duration>,
    capture_tx_slot: Arc<Mutex<Option<Sender<CaptureEvent>>>>,
    capture_device: Option<String>,
    capture: Option<Box<dyn CaptureBackend>>,
    dictionary_terms: Vec<String>,
    /// The profile's language, as read into `TranscribeOptions.language` by `start_capture`.
    /// Defaults to `None` like every other fixture; the one test that cares sets a real value
    /// and asserts it reached `begin_opts`, since this is the single hop that makes a profile's
    /// language reach the engine at all.
    language: Option<String>,
    begin_opts: Arc<Mutex<Vec<TranscribeOptions>>>,
    /// The profile's optional draft engine, whose partials drive the HUD preview while the user
    /// is still speaking. `None` (the default, and what every pre-existing test uses) means the
    /// profile has no preview at all, exactly as before this field existed.
    draft_engine: Option<Box<dyn SttEngine>>,
    /// Replaces the `FakeSttEngine` `build` would otherwise assemble from `engine_result`,
    /// `calls`, `partial`, `finish_delay` and `begin_opts` -- for the one case those fields
    /// cannot express, a *final* engine whose `begin` fails. Deliberately an override rather
    /// than another flag on `FakeSttEngine`: a failure switch defaulted to "off" on the fixture
    /// every other test shares is exactly the kind of field that sits at its default forever and
    /// makes a test green for a reason it never claimed.
    engine: Option<Box<dyn SttEngine>>,
}

impl DepsBuilder {
    fn new(engine_result: Result<Transcript, SttError>) -> Self {
        Self {
            mode: DictationMode::PushToTalk,
            refine_enabled: false,
            engine_result,
            calls: Arc::new(Mutex::new(Vec::new())),
            partial: None,
            finish_delay: Duration::ZERO,
            refiner: None,
            inject_fail: false,
            injection_method: InjectionMethod::Type,
            automatic_paste_expected: false,
            injected: Arc::new(Mutex::new(Vec::new())),
            rules: Vec::new(),
            snippets: Vec::new(),
            history: None,
            silence: None,
            capture_tx_slot: Arc::new(Mutex::new(None)),
            capture_device: None,
            capture: None,
            dictionary_terms: Vec::new(),
            language: None,
            begin_opts: Arc::new(Mutex::new(Vec::new())),
            draft_engine: None,
            engine: None,
        }
    }

    fn build(self, hotkey_rx: Receiver<HotkeyEvent>) -> RuntimeDeps {
        let refiner: Option<Arc<dyn TextRefiner>> = self.refiner.map(|(behavior, calls)| {
            Arc::new(FakeRefiner { behavior, calls }) as Arc<dyn TextRefiner>
        });

        let engine: Box<dyn SttEngine> = match self.engine {
            Some(engine) => engine,
            None => Box::new(FakeSttEngine {
                result: self.engine_result,
                calls: self.calls,
                partial: self.partial,
                finish_delay: self.finish_delay,
                begin_opts: self.begin_opts,
            }),
        };

        let profile_deps = ProfileDeps {
            engine,
            draft_engine: self.draft_engine,
            refiner,
            refine_enabled: self.refine_enabled,
            tone: Tone::Clean,
            language: self.language,
            engine_label: "fake-engine".to_string(),
            profile_id: "default".to_string(),
            initial_prompt: (!self.dictionary_terms.is_empty())
                .then(|| self.dictionary_terms.join(", ")),
        };
        let profiles = registry_with(vec![(test_profile("default"), profile_deps)]);

        let capture = self.capture.unwrap_or_else(|| {
            Box::new(FakeCaptureBackend {
                tx_slot: self.capture_tx_slot,
            })
        });

        RuntimeDeps {
            mode: self.mode,
            silence: self.silence,
            profiles,
            injector: Box::new(FakeInjector {
                injected: self.injected,
                fail: self.inject_fail,
                method: self.injection_method,
            }),
            automatic_paste_expected: self.automatic_paste_expected,
            rules: self.rules,
            snippets: self.snippets,
            history: self.history,
            capture_device: self.capture_device,
            capture,
            hotkey_rx,
            vad_sensitivity: 0.5,
            refine_timeout: Duration::from_secs(1),
        }
    }
}

fn fake_sink() -> (Arc<FakeSink>, Receiver<Emission>, Receiver<Notice>) {
    let (states_tx, states_rx) = unbounded();
    let (notices_tx, notices_rx) = unbounded();
    let sink = Arc::new(FakeSink {
        states_tx,
        notices_tx,
    });
    (sink, states_rx, notices_rx)
}

/// Retrieves the `Sender<CaptureEvent>` a `FakeCaptureBackend` stashed once
/// capture started.
///
/// The runtime now opens capture before emitting the initial `"recording"`
/// state, but keep this bounded poll: callers also use the helper around
/// reload/failure paths, and a timing assumption adds no value to those tests.
fn capture_tx(slot: &Arc<Mutex<Option<Sender<CaptureEvent>>>>) -> Sender<CaptureEvent> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(tx) = slot.lock().expect("lock").clone() {
            return tx;
        }
        assert!(
            Instant::now() < deadline,
            "capture should have started and stashed its sender within {WAIT:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_capture_calls(calls: &Arc<Mutex<Vec<Option<String>>>>, expected: usize) {
    let deadline = Instant::now() + WAIT;
    loop {
        if calls.lock().expect("lock").len() >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected} capture start calls within {WAIT:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// Builds a `ProfileDeps` whose engine always returns `text` from `finish()`, with no refiner --
/// the shape most routing tests need, where only the *identity* of the profile's output matters.
fn profile_deps_with_transcript(text: &str) -> ProfileDeps {
    ProfileDeps {
        engine: Box::new(FakeSttEngine {
            result: Ok(transcript(text)),
            calls: Arc::new(Mutex::new(Vec::new())),
            partial: None,
            finish_delay: Duration::ZERO,
            begin_opts: Arc::new(Mutex::new(Vec::new())),
        }),
        draft_engine: None,
        refiner: None,
        refine_enabled: false,
        tone: Tone::Clean,
        language: None,
        engine_label: "fake-engine".to_string(),
        profile_id: "fake-profile".to_string(),
        initial_prompt: None,
    }
}

/// Like `profile_deps_with_transcript`, but with `profile_id` set to `id` instead of the shared
/// `"fake-profile"` placeholder every routing test uses. Needed by any test asserting on the id a
/// history row was attributed to: two profiles sharing that placeholder would make such an
/// assertion unable to tell them apart, and `"fake-profile"` itself is just as unable as
/// `"default"` to distinguish "the pressed profile's own id" from "a hardcoded string".
fn profile_deps_with_transcript_and_id(text: &str, id: &str) -> ProfileDeps {
    ProfileDeps {
        profile_id: id.to_string(),
        ..profile_deps_with_transcript(text)
    }
}

/// Drives one full no-refine `PushToTalk` session for `binding` (press, release, and the
/// resulting state sequence) -- the common shape `each_hotkey_dictates_with_its_own_profile` and
/// `pressing_an_unregistered_binding_starts_no_session`'s companion tests both need.
fn press_and_release(
    hotkey_tx: &Sender<HotkeyEvent>,
    states_rx: &Receiver<Emission>,
    binding: BindingId,
) {
    hotkey_tx
        .send(HotkeyEvent::Pressed { binding })
        .expect("send pressed");
    assert_eq!(recv_state(states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released { binding })
        .expect("send released");
    assert_eq!(recv_state(states_rx), "transcribing");
    assert_eq!(recv_state(states_rx), "injecting");
    assert_eq!(recv_state(states_rx), "idle");
}

// ---- tests --------------------------------------------------------------

#[test]
fn capture_starts_before_the_recording_phase_reaches_the_hud() {
    let capture_started = Arc::new(AtomicBool::new(false));
    let (recording_tx, recording_rx) = unbounded();
    let sink = Arc::new(CaptureOrderSink {
        capture_started: capture_started.clone(),
        recording_tx,
    });

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("unused")));
    builder.capture = Some(Box::new(CaptureOrderBackend {
        started: capture_started,
    }));
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");

    assert!(
        recording_rx
            .recv_timeout(WAIT)
            .expect("expected recording phase"),
        "capture must be open before HUD/state work runs"
    );

    handle.shutdown();
}

#[test]
fn happy_path_emits_full_sequence_and_injects_refined_text() {
    let refine_calls = Arc::new(AtomicUsize::new(0));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.refine_enabled = true;
    builder.refiner = Some((RefineBehavior::Uppercase, refine_calls.clone()));
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "refining");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["HELLO WORLD"]);
    assert_eq!(refine_calls.load(Ordering::SeqCst), 1);

    // A dictation that worked says nothing. Every notice is now put in front
    // of the user by the desktop notification service, so a notice on this
    // path would be a popup over whatever they are typing into, once per
    // utterance -- worse than the silence it replaced.
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

#[test]
fn automatic_injection_reports_when_it_falls_back_to_clipboard_only() {
    let (sink, states_rx, notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("copied fallback")));
    builder.injection_method = InjectionMethod::ClipboardOnly;
    builder.automatic_paste_expected = true;

    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);
    press_and_release(&hotkey_tx, &states_rx, BindingId::from(0));

    let (kind, message) = recv_notice(&notices_rx);
    assert_eq!(kind, "warning");
    assert!(message.contains("copied to the clipboard"));
    assert!(message.contains("text-injection permission"));

    handle.shutdown();
}

#[test]
fn explicit_clipboard_only_does_not_warn_about_its_selected_behavior() {
    let (sink, states_rx, notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("copied by choice")));
    builder.injection_method = InjectionMethod::ClipboardOnly;
    builder.automatic_paste_expected = false;

    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);
    press_and_release(&hotkey_tx, &states_rx, BindingId::from(0));

    assert!(
        notices_rx.try_recv().is_err(),
        "clipboard-only is an explicit delivery mode, not a degradation"
    );

    handle.shutdown();
}

#[test]
fn refiner_failure_injects_raw_and_notifies() {
    let refine_calls = Arc::new(AtomicUsize::new(0));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.refine_enabled = true;
    builder.refiner = Some((
        RefineBehavior::Fail("refiner unreachable".to_string()),
        refine_calls.clone(),
    ));
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "refining");
    assert_eq!(recv_state(&states_rx), "injecting");

    // `RefineFailed` produces `[Inject(raw), NotifyInfo(..)]` (see
    // `on_refining` in session.rs): the notice is the *second* effect, run
    // only after `Inject`'s own nested dispatch has already emitted "idle".
    // So waiting for "idle" on `states_rx` proves nothing about whether the
    // notice has fired yet -- wait for it explicitly instead.
    let (kind, msg) = recv_notice(&notices_rx);
    assert_eq!(kind, "info");
    assert!(
        msg.contains("Refinement unavailable"),
        "expected a refinement-unavailable notice, got {msg:?}"
    );

    assert_eq!(recv_state(&states_rx), "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["hello world"]);
    assert_eq!(refine_calls.load(Ordering::SeqCst), 1);

    handle.shutdown();
}

#[test]
fn dictionary_rule_applied_before_injection_and_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("history.sqlite3");
    let history = HistoryRepo::open(&db_path).expect("open history db");

    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("open the pod bay doors")));
    builder.rules = vec![ReplaceRule {
        heard: "pod".to_string(),
        write: "airlock".to_string(),
    }];
    builder.injected = injected.clone();
    builder.history = Some(history);
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    assert_eq!(
        *injected.lock().expect("lock"),
        vec!["open the airlock bay doors"],
        "the dictionary rule must be applied to the raw transcript before injection"
    );

    handle.shutdown();

    let verify = HistoryRepo::open(&db_path).expect("reopen history db");
    let entries = verify.list(None, 10).expect("list history");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].raw_text, "open the pod bay doors");
    assert_eq!(entries[0].final_text, "open the airlock bay doors");
    assert_eq!(
        entries[0].profile_id.as_deref(),
        Some("default"),
        "the history row must be attributed to the profile that produced it"
    );
}

/// Also pins the profile's `language` reaching `TranscribeOptions` (every other fixture in this
/// file leaves it `None`, which made `start_capture` discarding it undetectable -- see
/// `start_capture`'s use of `deps.language`).
#[test]
fn dictionary_terms_are_passed_to_engine_as_initial_prompt() {
    let begin_opts = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.dictionary_terms = vec!["SQLite".to_string(), "Tauri".to_string()];
    builder.language = Some("ru".to_string());
    builder.begin_opts = begin_opts.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    let opts = begin_opts.lock().expect("lock");
    assert_eq!(opts.len(), 1);
    assert_eq!(opts[0].initial_prompt, Some("SQLite, Tauri".to_string()));
    assert_eq!(
        opts[0].language,
        Some("ru".to_string()),
        "the profile's own language must reach the engine's TranscribeOptions -- the single hop \
         that makes a hotkey's profile dictate in its configured language"
    );
    drop(opts);

    handle.shutdown();
}

#[test]
fn empty_dictionary_terms_produce_no_initial_prompt() {
    let begin_opts = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.begin_opts = begin_opts.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    let opts = begin_opts.lock().expect("lock");
    assert_eq!(opts.len(), 1);
    assert_eq!(opts[0].initial_prompt, None);
    drop(opts);

    handle.shutdown();
}

#[test]
fn snippet_trigger_bypasses_refiner() {
    let refine_calls = Arc::new(AtomicUsize::new(0));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("insert my signature")));
    builder.refine_enabled = true;
    builder.refiner = Some((RefineBehavior::Uppercase, refine_calls.clone()));
    builder.snippets = vec![Snippet {
        trigger: "insert my signature".to_string(),
        body: "John Doe, CEO".to_string(),
    }];
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "refining");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["John Doe, CEO"]);
    assert_eq!(
        refine_calls.load(Ordering::SeqCst),
        0,
        "the refiner must never be called for a snippet hit"
    );

    handle.shutdown();
}

#[test]
fn cancel_during_recording_injects_nothing() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("should never be seen")));
    builder.calls = calls.clone();
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    handle.cancel();
    assert_eq!(recv_state(&states_rx), "idle");

    assert!(injected.lock().expect("lock").is_empty());
    assert!(
        calls.lock().expect("lock").is_empty(),
        "engine.feed()/finish() must never run for a cancelled recording with no audio"
    );
    assert_no_more_states(&states_rx);

    handle.shutdown();
}

#[test]
fn cancel_after_finish_before_transcript_ready_injects_nothing() {
    // Commit point 1/2 (see the module doc comment in runtime.rs): a cancel
    // that arrives while `engine.finish()` is blocking must still prevent
    // injection. Delaying `finish()` gives the test a real-time window to
    // send `cancel()` after "transcribing" is observed (which happens
    // *before* `finish()` is even called) but comfortably before `finish()`
    // returns and the runtime checks for a pending cancel.
    let injected = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.finish_delay = Duration::from_millis(250);
    builder.calls = calls.clone();
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");

    handle.cancel();

    assert_eq!(recv_state(&states_rx), "idle");
    assert!(injected.lock().expect("lock").is_empty());
    assert_no_more_states(&states_rx);

    handle.shutdown();
}

/// A draft result can already be available while the authoritative engine is
/// still finishing. Cancellation at that boundary suppresses both injection
/// and the flushed HUD preview: accessory text must not be emitted after the
/// user has cancelled merely because its worker won the race.
#[test]
fn cancel_before_transcript_ready_suppresses_flushed_draft_preview() {
    let final_calls = Arc::new(Mutex::new(Vec::new()));
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (final_started_tx, final_started_rx) = unbounded();
    let (final_release_tx, final_release_rx) = unbounded();
    let (final_returned_tx, final_returned_rx) = unbounded();
    let (draft_started_tx, draft_started_rx) = unbounded();
    let (draft_release_tx, draft_release_rx) = unbounded();
    let (draft_returned_tx, draft_returned_rx) = unbounded();
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("unused fixture result")));
    builder.engine = Some(Box::new(GatedFinishSttEngine {
        calls: final_calls,
        started_tx: final_started_tx,
        release_rx: final_release_rx,
        returned_tx: final_returned_tx,
    }));
    builder.draft_engine = Some(Box::new(GatedFinishSttEngine {
        calls: draft_calls,
        started_tx: draft_started_tx,
        release_rx: draft_release_rx,
        returned_tx: draft_returned_tx,
    }));
    builder.injected = injected.clone();
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    final_started_rx
        .recv_timeout(WAIT)
        .expect("authoritative finish should reach its gate");
    draft_started_rx
        .recv_timeout(WAIT)
        .expect("draft finish should reach its gate");

    // Make the draft result genuinely ready before cancellation. Without
    // this handshake the test could pass merely because there was no flushed
    // text available to suppress.
    let _ = draft_release_tx.send(());
    draft_returned_rx
        .recv_timeout(WAIT)
        .expect("released draft finish should return before cancel");

    handle.cancel();
    let _ = final_release_tx.send(());
    final_returned_rx
        .recv_timeout(WAIT)
        .expect("released authoritative finish should return");

    let mut partials_after_cancel = Vec::new();
    loop {
        let (state, _, partial) = states_rx
            .recv_timeout(WAIT)
            .expect("cancelled session should return to idle");
        if let Some(partial) = partial {
            partials_after_cancel.push(partial);
        }
        if state == "idle" {
            break;
        }
    }

    assert!(
        partials_after_cancel.is_empty(),
        "cancel must suppress the already-available flushed draft preview, got {partials_after_cancel:?}"
    );
    assert!(
        injected.lock().expect("lock").is_empty(),
        "cancel must suppress the authoritative transcript too"
    );
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

/// Cancelling while a draft flush is still inside native code must neither
/// wait nor discard that engine. Once its healthy late outcome is processed
/// after Idle, the same profile previews and finishes normally again without
/// a settings reload.
#[test]
fn cancelled_pending_draft_recovers_without_reload() {
    let final_calls = Arc::new(Mutex::new(Vec::new()));
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let capture_tx_slot = Arc::new(Mutex::new(None));
    let (final_started_tx, final_started_rx) = unbounded();
    let (final_release_tx, final_release_rx) = unbounded();
    let (final_returned_tx, final_returned_rx) = unbounded();
    let (draft_started_tx, draft_started_rx) = unbounded();
    let (draft_release_tx, draft_release_rx) = unbounded();
    let (draft_returned_tx, draft_returned_rx) = unbounded();
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("unused fixture result")));
    builder.engine = Some(Box::new(SilentGatedFinishSttEngine(GatedFinishSttEngine {
        calls: final_calls,
        started_tx: final_started_tx,
        release_rx: final_release_rx,
        returned_tx: final_returned_tx,
    })));
    builder.draft_engine = Some(Box::new(GatedFinishSttEngine {
        calls: draft_calls.clone(),
        started_tx: draft_started_tx,
        release_rx: draft_release_rx,
        returned_tx: draft_returned_tx,
    }));
    builder.injected = injected.clone();
    builder.capture_tx_slot = capture_tx_slot.clone();
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send first pressed");
    recv_until(&states_rx, "recording");
    capture_tx(&capture_tx_slot)
        .send(CaptureEvent::Frame(AudioFrame {
            samples: vec![31, 32, 33],
        }))
        .expect("send first frame");
    assert_eq!(recv_partial(&states_rx).as_deref(), Some("working preview"));
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send first released");
    recv_until(&states_rx, "transcribing");
    final_started_rx
        .recv_timeout(WAIT)
        .expect("first final finish should block");
    draft_started_rx
        .recv_timeout(WAIT)
        .expect("first draft finish should block");

    handle.cancel();
    let _ = final_release_tx.send(());
    final_returned_rx
        .recv_timeout(WAIT)
        .expect("cancelled final finish should return");

    let mut partials_after_cancel = Vec::new();
    loop {
        let (state, _, partial) = states_rx
            .recv_timeout(WAIT)
            .expect("cancelled pending session should return to idle");
        if let Some(partial) = partial {
            partials_after_cancel.push(partial);
        }
        if state == "idle" {
            break;
        }
    }
    assert!(
        partials_after_cancel.is_empty(),
        "cancel must never emit the pending flushed preview, got {partials_after_cancel:?}"
    );
    assert!(injected.lock().expect("lock").is_empty());
    assert_no_more_notices(&notices_rx);

    // Release only after Idle, then wait for both native return and observable
    // runtime recovery. The bounded probe handles the tiny channel/select
    // race by cancelling an empty-preview probe and trying after its Idle
    // boundary; success itself proves the outcome was processed and restored.
    let _ = draft_release_tx.send(());
    draft_returned_rx
        .recv_timeout(WAIT)
        .expect("cancelled draft worker should return after release");
    let recovery_deadline = Instant::now() + WAIT;
    loop {
        hotkey_tx
            .send(HotkeyEvent::Pressed {
                binding: BindingId::from(0),
            })
            .expect("send recovery-probe pressed");
        recv_until(&states_rx, "recording");
        capture_tx(&capture_tx_slot)
            .send(CaptureEvent::Frame(AudioFrame {
                samples: vec![41, 42, 43],
            }))
            .expect("send recovery-probe frame");
        if recv_partial_before(&states_rx, Duration::from_millis(100)).as_deref()
            == Some("working preview")
        {
            break;
        }
        handle.cancel();
        recv_until(&states_rx, "idle");
        assert!(
            Instant::now() < recovery_deadline,
            "cancelled draft outcome should restore this profile within {WAIT:?}"
        );
    }

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send recovered-profile released");
    recv_until(&states_rx, "transcribing");
    final_started_rx
        .recv_timeout(WAIT)
        .expect("recovered final finish should start");
    draft_started_rx
        .recv_timeout(WAIT)
        .expect("recovered draft should finish without reload");
    let _ = draft_release_tx.send(());
    let _ = final_release_tx.send(());
    draft_returned_rx
        .recv_timeout(WAIT)
        .expect("recovered draft finish should return");
    final_returned_rx
        .recv_timeout(WAIT)
        .expect("recovered final finish should return");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["late draft result"]);
    assert_eq!(
        draft_calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|call| **call == CallRecord::Finish)
            .count(),
        2,
        "the cancelled worker and recovered session should each finish once"
    );
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

/// Cancellation itself is silent, but it must not hide a genuine model
/// failure that becomes known later. The eventual error removes preview and
/// produces exactly one informational notice without reviving cancelled HUD
/// or injected text.
#[test]
fn late_draft_failure_after_cancel_is_reported_once() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (final_started_tx, final_started_rx) = unbounded();
    let (final_release_tx, final_release_rx) = unbounded();
    let (final_returned_tx, final_returned_rx) = unbounded();
    let (draft_started_tx, draft_started_rx) = unbounded();
    let (draft_release_tx, draft_release_rx) = unbounded();
    let (draft_returned_tx, draft_returned_rx) = unbounded();
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("unused fixture result")));
    builder.engine = Some(Box::new(GatedFinishSttEngine {
        calls: Arc::new(Mutex::new(Vec::new())),
        started_tx: final_started_tx,
        release_rx: final_release_rx,
        returned_tx: final_returned_tx,
    }));
    builder.draft_engine = Some(Box::new(GatedFailingFinishSttEngine {
        calls: Arc::new(Mutex::new(Vec::new())),
        started_tx: draft_started_tx,
        release_rx: draft_release_rx,
        returned_tx: draft_returned_tx,
        dropped_tx: None,
    }));
    builder.injected = injected.clone();
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    recv_until(&states_rx, "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    recv_until(&states_rx, "transcribing");
    final_started_rx
        .recv_timeout(WAIT)
        .expect("final finish should block");
    draft_started_rx
        .recv_timeout(WAIT)
        .expect("draft finish should block");

    handle.cancel();
    let _ = final_release_tx.send(());
    final_returned_rx
        .recv_timeout(WAIT)
        .expect("cancelled final finish should return");
    recv_until(&states_rx, "idle");
    assert!(injected.lock().expect("lock").is_empty());
    assert_no_more_notices(&notices_rx);

    let _ = draft_release_tx.send(());
    draft_returned_rx
        .recv_timeout(WAIT)
        .expect("late failing draft should return");
    assert_preview_lost_notice(&notices_rx);
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

#[test]
fn cancel_during_refine_injects_nothing() {
    // Commit point 2/2: a cancel that arrives while the refine call is in
    // flight must still prevent injection, even though the refine call
    // itself completed (the refiner's own network/inference call cannot be
    // aborted mid-flight — only the resulting `Inject` is prevented). This
    // is also the only place a "cancel just before inject" could land in
    // this design: nothing async happens between a refine call resolving
    // and the `Inject` effect it would produce, so there is no separate,
    // distinguishable commit point to test beyond this one.
    let injected = Arc::new(Mutex::new(Vec::new()));
    let refine_calls = Arc::new(AtomicUsize::new(0));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.refine_enabled = true;
    builder.refiner = Some((
        RefineBehavior::Delay(Duration::from_millis(250)),
        refine_calls,
    ));
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "refining");

    handle.cancel();

    assert_eq!(recv_state(&states_rx), "idle");
    assert!(
        injected.lock().expect("lock").is_empty(),
        "nothing must be injected once a cancel arrived before the inject commit point"
    );
    assert_no_more_states(&states_rx);

    handle.shutdown();
}

#[test]
fn empty_transcript_notifies_and_injects_nothing() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("   ")));
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "idle");

    // Absence assertion: checked directly, not awaited -- there's nothing to
    // wait for here, only a Vec that must still be empty.
    assert!(injected.lock().expect("lock").is_empty());

    // `TranscriptReady` with empty text produces `[NotifyInfo(..)]` after the
    // transition to "idle" is already emitted (see `on_transcribing` in
    // session.rs), so the notice can genuinely still be in flight here.
    // Wait for it explicitly instead of inferring it already happened.
    let (kind, msg) = recv_notice(&notices_rx);
    assert_eq!(kind, "info");
    assert_eq!(msg, "Nothing heard");

    handle.shutdown();
}

#[test]
fn history_entry_recorded_with_raw_and_final_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("history.sqlite3");
    let history = HistoryRepo::open(&db_path).expect("open history db");

    let refine_calls = Arc::new(AtomicUsize::new(0));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.refine_enabled = true;
    builder.refiner = Some((RefineBehavior::Uppercase, refine_calls));
    builder.injected = injected.clone();
    builder.history = Some(history);
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "refining");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["HELLO WORLD"]);

    // A dictation that worked says nothing -- asserted here as well as on the
    // happy path, because this is the only test that exercises the history
    // write, and a write that failed (or that decided to report itself as it
    // went) would announce it through this channel.
    assert_no_more_notices(&notices_rx);

    handle.shutdown();

    // Re-open the same on-disk database to verify what the worker thread
    // persisted; the write is autocommit and happened-before the "idle"
    // emission above, so it is guaranteed visible here.
    let verify = HistoryRepo::open(&db_path).expect("reopen history db");
    let entries = verify.list(None, 10).expect("list history");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].raw_text, "hello world");
    assert_eq!(entries[0].final_text, "HELLO WORLD");
    assert_eq!(entries[0].engine, "fake-engine");
    assert!(entries[0].duration_ms >= 0);
}

#[test]
fn reload_swaps_deps_between_sessions() {
    let injected_a = Arc::new(Mutex::new(Vec::new()));
    let injected_b = Arc::new(Mutex::new(Vec::new()));
    let begin_opts_b = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder_a = DepsBuilder::new(Ok(transcript("hello world")));
    builder_a.injected = injected_a.clone();
    let deps_a = builder_a.build(hotkey_rx);

    let handle = Runtime::spawn(deps_a, sink);

    // Still idle (no session started yet): the new deps, including a fresh
    // hotkey channel and updated dictionary terms, apply immediately rather
    // than being queued.
    let (hotkey_tx_b, hotkey_rx_b) = unbounded();
    let mut builder_b = DepsBuilder::new(Ok(transcript("second session")));
    builder_b.injected = injected_b.clone();
    builder_b.dictionary_terms = vec!["SQLite".to_string(), "Tauri".to_string()];
    builder_b.begin_opts = begin_opts_b.clone();
    let deps_b = builder_b.build(hotkey_rx_b);
    handle.reload(deps_b);

    hotkey_tx_b
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx_b
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    assert!(injected_a.lock().expect("lock").is_empty());
    assert_eq!(*injected_b.lock().expect("lock"), vec!["second session"]);

    // The reloaded deps' dictionary terms must reach the STT engine as an
    // `initial_prompt`, proving `WorkerCtx::apply` copies `profiles`
    // just like every other field (not just at `WorkerCtx::new`).
    let opts_b = begin_opts_b.lock().expect("lock");
    assert_eq!(opts_b.len(), 1);
    assert_eq!(opts_b[0].initial_prompt, Some("SQLite, Tauri".to_string()));
    drop(opts_b);

    // The old hotkey channel's receiver was dropped by `reload`; sending on
    // it now simply fails rather than resurrecting the old session.
    let _ = hotkey_tx.send(HotkeyEvent::Pressed {
        binding: BindingId::from(0),
    });

    handle.shutdown();
}

#[test]
fn toggle_drives_a_full_session_in_toggle_mode() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (_hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("toggled")));
    builder.mode = DictationMode::Toggle;
    builder.injected = injected.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    handle.toggle();
    assert_eq!(recv_state(&states_rx), "recording");

    handle.toggle();
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["toggled"]);

    handle.shutdown();
}

#[test]
fn stream_failure_during_recording_returns_idle_and_injects_nothing() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let mut builder = DepsBuilder::new(Ok(transcript("partial words must be discarded")));
    builder.calls = calls.clone();
    builder.injected = injected.clone();
    builder.capture_tx_slot = tx_slot.clone();
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    capture_tx(&tx_slot)
        .send(CaptureEvent::StreamFailed(
            "Bluetooth device disconnected".to_string(),
        ))
        .expect("send stream failure");

    recv_until(&states_rx, "idle");
    let (kind, message) = recv_notice(&notices_rx);
    assert_eq!(kind, "error");
    assert!(message.contains("Bluetooth device disconnected"));
    assert!(message.contains("press the hotkey to try again"));
    assert!(injected.lock().expect("lock").is_empty());
    assert!(
        !calls.lock().expect("lock").contains(&CallRecord::Finish),
        "capture failure must cancel, not transcribe a partial utterance"
    );

    handle.shutdown();
}

#[test]
fn next_press_creates_a_fresh_capture_after_stream_failure() {
    let starts = Arc::new(AtomicUsize::new(0));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let mut builder = DepsBuilder::new(Ok(transcript("second attempt")));
    builder.capture = Some(Box::new(CountingCaptureBackend {
        starts: starts.clone(),
        tx_slot: tx_slot.clone(),
    }));
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send first press");
    assert_eq!(recv_state(&states_rx), "recording");
    let failed_tx = capture_tx(&tx_slot);
    failed_tx
        .send(CaptureEvent::StreamFailed("device vanished".to_string()))
        .expect("send stream failure");
    recv_until(&states_rx, "idle");
    let _ = recv_notice(&notices_rx);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send second press");
    assert_eq!(recv_state(&states_rx), "recording");
    let deadline = Instant::now() + WAIT;
    while starts.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < deadline, "second capture did not start");
        thread::sleep(Duration::from_millis(5));
    }

    handle.cancel();
    recv_until(&states_rx, "idle");
    handle.shutdown();
}

#[test]
fn late_stream_failure_after_stop_does_not_finish_or_notify_twice() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let mut builder = DepsBuilder::new(Ok(transcript("complete utterance")));
    builder.calls = calls.clone();
    builder.injected = injected.clone();
    builder.finish_delay = Duration::from_millis(100);
    builder.capture_tx_slot = tx_slot.clone();
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    let stale_tx = capture_tx(&tx_slot);

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    recv_until(&states_rx, "transcribing");
    stale_tx
        .send(CaptureEvent::StreamFailed("late callback".to_string()))
        .expect("send late failure");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["complete utterance"]);
    assert_eq!(
        calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|call| **call == CallRecord::Finish)
            .count(),
        1
    );
    assert_no_more_states(&states_rx);
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

#[test]
fn missing_selected_device_falls_back_once_without_rewriting_the_preference() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let mut builder = DepsBuilder::new(Ok(transcript("fallback works")));
    builder.capture_device = Some("Andrey's AirPods".to_string());
    builder.capture = Some(Box::new(MissingSelectedCaptureBackend {
        calls: calls.clone(),
        tx_slot,
        default_error: None,
    }));
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send first press");
    assert_eq!(recv_state(&states_rx), "recording");
    wait_for_capture_calls(&calls, 2);
    let (kind, message) = recv_notice(&notices_rx);
    assert_eq!(kind, "warning");
    assert!(message.contains("Andrey's AirPods"));
    assert!(message.contains("system default for this run"));
    assert!(message.contains("saved device was not changed"));

    handle.cancel();
    recv_until(&states_rx, "idle");

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send second press");
    assert_eq!(recv_state(&states_rx), "recording");
    wait_for_capture_calls(&calls, 3);
    assert_eq!(
        *calls.lock().expect("lock"),
        vec![Some("Andrey's AirPods".to_string()), None, None,],
        "the unavailable saved name is retried only after a reload/restart"
    );
    assert_no_more_notices(&notices_rx);

    handle.cancel();
    recv_until(&states_rx, "idle");
    handle.shutdown();
}

#[test]
fn failed_default_fallback_ends_the_session_with_an_actionable_error() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let mut builder = DepsBuilder::new(Ok(transcript("must not be inserted")));
    builder.injected = injected.clone();
    builder.capture_device = Some("Unplugged USB Mic".to_string());
    builder.capture = Some(Box::new(MissingSelectedCaptureBackend {
        calls: calls.clone(),
        tx_slot: Arc::new(Mutex::new(None)),
        default_error: Some(AudioError::NoDefaultDevice),
    }));
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(
        recv_state(&states_rx),
        "idle",
        "a microphone that never opened must not be reported as recording"
    );
    assert_no_more_states(&states_rx);

    let (fallback_kind, fallback_message) = recv_notice(&notices_rx);
    assert_eq!(fallback_kind, "warning");
    assert!(fallback_message.contains("system default"));
    let (error_kind, error_message) = recv_notice(&notices_rx);
    assert_eq!(error_kind, "error");
    assert!(error_message.contains("no default input device"));
    assert!(error_message.contains("connected and selected"));
    assert_eq!(
        *calls.lock().expect("lock"),
        vec![Some("Unplugged USB Mic".to_string()), None]
    );
    assert!(injected.lock().expect("lock").is_empty());

    handle.shutdown();
}

#[test]
fn audio_frames_are_fed_to_engine_and_partial_reaches_sink() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.calls = calls.clone();
    builder.partial = Some("partial text".to_string());
    builder.capture_tx_slot = tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    let tx = capture_tx(&tx_slot);
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![100; 50],
    }))
    .expect("send frame");

    // The frame's rms/partial reaches the sink as a "recording" emission
    // carrying the scripted partial.
    let partial = recv_partial(&states_rx);
    assert_eq!(partial.as_deref(), Some("partial text"));

    // ... and the frame's samples actually reached `engine.feed`.
    let calls = calls.lock().expect("lock");
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, CallRecord::Feed(samples) if samples.len() == 50)),
        "expected engine.feed to have been called with the pushed frame, got {calls:?}"
    );
    drop(calls);

    handle.shutdown();
}

#[test]
fn trailing_frames_are_fed_before_finish_is_called() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.calls = calls.clone();
    builder.capture_tx_slot = tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    let tx = capture_tx(&tx_slot);
    // Push frames and release the hotkey back-to-back, without waiting for
    // either frame to be individually processed first: whichever the
    // `select!` loop happens to pick up first (an audio frame via the
    // normal recording-path feed, or the hotkey release triggering
    // `StopCapture`'s trailing-frame drain), every sample must still reach
    // `engine.feed` strictly before `engine.finish()` is called.
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![1, 2, 3],
    }))
    .expect("send frame 1");
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![4, 5, 6],
    }))
    .expect("send frame 2");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");

    // Depending on which the `select!` loop happens to service first, zero,
    // one, or two of the pushed frames may be processed via the normal
    // recording-path feed (each emitting its own extra "recording") before
    // `Released` is picked up — `recv_until` skips those rather than
    // asserting an exact count, since the ordering guarantee under test is
    // about `engine.feed`/`engine.finish()` call order, not the state
    // channel's exact cardinality.
    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    let calls = calls.lock().expect("lock");
    let finish_index = calls
        .iter()
        .position(|c| *c == CallRecord::Finish)
        .expect("finish() should have been called exactly once");
    let feeds_before_finish: Vec<&CallRecord> = calls[..finish_index].iter().collect();
    assert_eq!(
        feeds_before_finish.len(),
        2,
        "both frames should have been fed before finish(), got {calls:?}"
    );
    assert!(feeds_before_finish
        .iter()
        .any(|c| matches!(c, CallRecord::Feed(s) if s == &vec![1i16, 2, 3])));
    assert!(feeds_before_finish
        .iter()
        .any(|c| matches!(c, CallRecord::Feed(s) if s == &vec![4i16, 5, 6])));
    drop(calls);

    handle.shutdown();
}

#[test]
fn silence_timeout_stops_recording_without_hotkey_release() {
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (_hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.silence = Some(Duration::from_millis(30));
    builder.capture_tx_slot = tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    // Drive recording via `toggle` rather than a hotkey channel, since no
    // release is ever going to be sent in this test.
    handle.toggle();
    assert_eq!(recv_state(&states_rx), "recording");

    let tx = capture_tx(&tx_slot);
    // All-zero samples are silence (rms 0.0). Spaced 10ms apart in real
    // time so the 30ms silence hold genuinely elapses; comfortably more
    // frames than needed, for margin.
    for _ in 0..10 {
        tx.send(CaptureEvent::Frame(AudioFrame {
            samples: vec![0i16; 10],
        }))
        .expect("send silent frame");
        thread::sleep(Duration::from_millis(10));
    }

    // No hotkey release, no manual cancel: only the silence timeout can
    // have driven this transition.
    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    handle.shutdown();
}

/// Pins the whole point of per-profile routing: which profile a press resolves to must actually
/// be driven by its `BindingId`, not just "whichever engine happened to load first". The two
/// profiles are
/// deliberately given *different* output text (Russian vs. English) rather than the same text --
/// a routing bug that always used binding 0's profile, or that shared one `Session`/engine across
/// bindings, would still pass a test whose profiles produced identical text. Both directions are
/// asserted (press 1 then 0, not just 0 then 1) so a bug that only gets the *first* press right
/// (e.g. one that latches onto whichever profile started the worker) cannot slip through.
#[test]
fn each_hotkey_dictates_with_its_own_profile() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let profiles = registry_with(vec![
        (test_profile("ru"), profile_deps_with_transcript("привет")),
        (test_profile("en"), profile_deps_with_transcript("hello")),
    ]);

    let deps = RuntimeDeps {
        mode: DictationMode::PushToTalk,
        silence: None,
        profiles,
        injector: Box::new(FakeInjector {
            injected: injected.clone(),
            fail: false,
            method: InjectionMethod::Type,
        }),
        automatic_paste_expected: false,
        rules: Vec::new(),
        snippets: Vec::new(),
        history: None,
        capture_device: None,
        capture: Box::new(FakeCaptureBackend {
            tx_slot: Arc::new(Mutex::new(None)),
        }),
        hotkey_rx,
        vad_sensitivity: 0.5,
        refine_timeout: Duration::from_secs(1),
    };

    let handle = Runtime::spawn(deps, sink);

    press_and_release(&hotkey_tx, &states_rx, BindingId::from(1));
    assert_eq!(
        injected.lock().expect("lock").last(),
        Some(&"hello".to_string()),
        "binding 1 must dictate with the \"en\" profile, loaded lazily on this first press"
    );

    press_and_release(&hotkey_tx, &states_rx, BindingId::from(0));
    assert_eq!(
        injected.lock().expect("lock").last(),
        Some(&"привет".to_string()),
        "binding 0 must dictate with the \"ru\" profile, not stay latched on binding 1's"
    );

    handle.shutdown();
}

/// Pins the `Session::new`-at-press-time fix the task brief calls out: `refine_enabled` is a
/// per-profile value now, so a session started by one binding must not carry over whatever the
/// *previous* binding's flag was. Binding 0's profile has refinement off, binding 1's has it on
/// (with a real, distinguishable refiner); pressing 0 then 1 proves the flag is read fresh at
/// each press rather than fixed for the worker's whole lifetime.
#[test]
fn each_profile_applies_its_own_refine_enabled_flag_at_press_time() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let refine_calls = Arc::new(AtomicUsize::new(0));
    let (sink, states_rx, _notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let off_deps = profile_deps_with_transcript("plain");

    let mut on_deps = profile_deps_with_transcript("fancy");
    on_deps.refine_enabled = true;
    on_deps.refiner = Some(Arc::new(FakeRefiner {
        behavior: RefineBehavior::Uppercase,
        calls: refine_calls.clone(),
    }));

    let profiles = registry_with(vec![
        (test_profile("off"), off_deps),
        (test_profile("on"), on_deps),
    ]);

    let deps = RuntimeDeps {
        mode: DictationMode::PushToTalk,
        silence: None,
        profiles,
        injector: Box::new(FakeInjector {
            injected: injected.clone(),
            fail: false,
            method: InjectionMethod::Type,
        }),
        automatic_paste_expected: false,
        rules: Vec::new(),
        snippets: Vec::new(),
        history: None,
        capture_device: None,
        capture: Box::new(FakeCaptureBackend {
            tx_slot: Arc::new(Mutex::new(None)),
        }),
        hotkey_rx,
        vad_sensitivity: 0.5,
        refine_timeout: Duration::from_secs(1),
    };

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");
    assert_eq!(
        injected.lock().expect("lock").last(),
        Some(&"plain".to_string())
    );
    assert_eq!(
        refine_calls.load(Ordering::SeqCst),
        0,
        "binding 0's profile has refinement off"
    );

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(1),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(1),
        })
        .expect("send released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    assert_eq!(recv_state(&states_rx), "refining");
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");
    assert_eq!(
        injected.lock().expect("lock").last(),
        Some(&"FANCY".to_string())
    );
    assert_eq!(
        refine_calls.load(Ordering::SeqCst),
        1,
        "binding 1's profile has refinement on and must actually run it"
    );

    handle.shutdown();
}

/// Pins the `session.state() == State::Idle` guard in `handle_hotkey_pressed` (see its module doc
/// comment): a binding is only ever (re)selected while the session is `Idle`. `Toggle` mode's
/// second press -- the one that stops recording -- need not come from the same binding that
/// started it (a user could, in principle, hit a different profile's chord mid-utterance); the
/// guard is what makes that press stop the in-flight session instead of being treated as a fresh
/// selection. Without it, `start_session_for` would run again, reconstructing `Session` back to
/// `Idle` and making `on_idle(HotkeyPressed)` start a *new* recording rather than stop the old
/// one -- so `Toggle`-mode dictation could never be stopped by its own hotkey, and the
/// overwritten `ctx.active_capture` would drop the first press's capture handle without
/// `stop()`, leaking its audio stream. Two profiles with deliberately different transcripts:
/// press binding 0 to start, then press binding 1 while `Recording`. The correct outcome is a
/// single `"transcribing"` emission (the *same* session stopping) and an injected transcript
/// from binding 0's profile -- binding 1 must never be consulted.
#[test]
fn second_binding_pressed_mid_recording_in_toggle_mode_stops_binding_zeros_session() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let profiles = registry_with(vec![
        (test_profile("zero"), profile_deps_with_transcript("first")),
        (test_profile("one"), profile_deps_with_transcript("second")),
    ]);

    let deps = RuntimeDeps {
        mode: DictationMode::Toggle,
        silence: None,
        profiles,
        injector: Box::new(FakeInjector {
            injected: injected.clone(),
            fail: false,
            method: InjectionMethod::Type,
        }),
        automatic_paste_expected: false,
        rules: Vec::new(),
        snippets: Vec::new(),
        history: None,
        capture_device: None,
        capture: Box::new(FakeCaptureBackend {
            tx_slot: Arc::new(Mutex::new(None)),
        }),
        hotkey_rx,
        vad_sensitivity: 0.5,
        refine_timeout: Duration::from_secs(1),
    };

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    // Binding 1, pressed while binding 0's session is `Recording` -- not `Idle` -- must not be
    // consulted at all (see `handle_hotkey_pressed`'s doc comment): this press just stops the
    // in-flight session.
    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(1),
        })
        .expect("send pressed");
    assert_eq!(
        recv_state(&states_rx),
        "transcribing",
        "the second press must stop binding 0's session, not rebuild it and start recording \
         again with binding 1's profile"
    );
    assert_eq!(recv_state(&states_rx), "injecting");
    assert_eq!(recv_state(&states_rx), "idle");

    assert_eq!(
        injected.lock().expect("lock").last(),
        Some(&"first".to_string()),
        "the utterance must still transcribe with binding 0's profile -- binding 1 was pressed \
         mid-recording and must never be consulted"
    );

    assert_no_more_states(&states_rx);

    handle.shutdown();
}

/// Pins that a history row is attributed to the profile that actually produced it, on a
/// registry where that is provable -- a *two*-profile registry whose ids are neither "default"
/// (the id `LanguageProfile::default()` and `DepsBuilder`'s single-profile fixture both happen to
/// use) nor `profile_deps_with_transcript`'s shared `"fake-profile"` placeholder. Both existing
/// history tests (`dictionary_rule_applied_before_injection_and_history`,
/// `history_entry_recorded_with_raw_and_final_text`) build a single-profile registry whose only
/// profile is named "default" and assert `profile_id == Some("default")`, which cannot
/// distinguish "the pressed profile's own id was copied" from "the string default was hardcoded"
/// -- the fixture-at-its-defaults shape this branch has hit repeatedly. Pressing binding 1 here
/// must write binding 1's id ("en"), not binding 0's ("ru") and not "default".
#[test]
fn history_entry_attributes_to_the_profile_that_was_actually_pressed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("history.sqlite3");
    let history = HistoryRepo::open(&db_path).expect("open history db");

    let (sink, states_rx, _notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let profiles = registry_with(vec![
        (
            test_profile("ru"),
            profile_deps_with_transcript_and_id("привет", "ru"),
        ),
        (
            test_profile("en"),
            profile_deps_with_transcript_and_id("hello", "en"),
        ),
    ]);

    let deps = RuntimeDeps {
        mode: DictationMode::PushToTalk,
        silence: None,
        profiles,
        injector: Box::new(FakeInjector {
            injected: Arc::new(Mutex::new(Vec::new())),
            fail: false,
            method: InjectionMethod::Type,
        }),
        automatic_paste_expected: false,
        rules: Vec::new(),
        snippets: Vec::new(),
        history: Some(history),
        capture_device: None,
        capture: Box::new(FakeCaptureBackend {
            tx_slot: Arc::new(Mutex::new(None)),
        }),
        hotkey_rx,
        vad_sensitivity: 0.5,
        refine_timeout: Duration::from_secs(1),
    };

    let handle = Runtime::spawn(deps, sink);

    press_and_release(&hotkey_tx, &states_rx, BindingId::from(1));

    handle.shutdown();

    let verify = HistoryRepo::open(&db_path).expect("reopen history db");
    let entries = verify.list(None, 10).expect("list history");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].profile_id.as_deref(),
        Some("en"),
        "the history row must be attributed to binding 1's profile (\"en\"), not binding 0's \
         (\"ru\") and not a hardcoded \"default\""
    );
}

/// `ProfileRegistry::deps_for` returning `None` means only one thing: no binding with that id
/// exists (see its doc comment). In production this is unreachable -- `create_source` is only
/// ever handed specs for bindings the registry also has entries for -- but the worker still
/// checks explicitly (`handle_hotkey_pressed`) rather than assuming, and this pins that a press
/// for an unknown id is dropped silently: no session starts, nothing crashes.
#[test]
fn pressing_an_unregistered_binding_starts_no_session() {
    let (sink, states_rx, _notices_rx) = fake_sink();
    let (hotkey_tx, hotkey_rx) = unbounded();

    let builder = DepsBuilder::new(Ok(transcript("hello world")));
    let deps = builder.build(hotkey_rx); // single-profile registry: only binding 0 exists.
    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(1),
        })
        .expect("send pressed");
    assert_no_more_states(&states_rx);

    handle.shutdown();
}

/// The fan-out itself, stated as the contract actually is: **while the session is recording**, a
/// profile's draft engine is fed every frame its final engine is, with the same samples in the
/// same order. (Frames drained after capture stops reach the final engine alone -- see
/// `stop_capture_and_maybe_transcribe` -- so the two feed logs are equal only over the recording
/// window.) Once that window closes, the draft is finished exactly once to flush its decoder.
///
/// The hotkey is released only once the draft engine has been observed to receive both frames,
/// which is what makes the assertion deterministic: the `select!` loop, not the test, decides
/// whether a pushed frame is serviced as a recording frame or left to the trailing drain, and
/// waiting on the call log settles that without a sleep. The two frames carry different samples,
/// so "same order" is falsifiable, and the draft engine's log length is asserted outright --
/// a prefix assertion alone would pass trivially against an empty log, which is precisely what a
/// deleted fan-out produces.
#[test]
fn both_engines_are_fed_the_same_frames_while_recording() {
    let final_calls = Arc::new(Mutex::new(Vec::new()));
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.calls = final_calls.clone();
    builder.capture_tx_slot = tx_slot.clone();
    builder.draft_engine = Some(draft_engine(
        "preview",
        draft_calls.clone(),
        Arc::new(Mutex::new(Vec::new())),
    ));
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    let tx = capture_tx(&tx_slot);
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![1, 2, 3],
    }))
    .expect("send frame 1");
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![4, 5, 6],
    }))
    .expect("send frame 2");

    // Both frames have now gone through the recording path -- and therefore through the
    // fan-out -- so releasing here cannot leave either of them to the trailing drain.
    wait_for_feeds(&draft_calls, 2);

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");

    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    let fed_to_draft = fed_samples(&draft_calls);
    assert_eq!(
        fed_to_draft,
        vec![vec![1i16, 2, 3], vec![4i16, 5, 6]],
        "the draft engine must be fed every frame the final engine is while recording, with the \
         same samples in the same order"
    );
    assert!(
        fed_samples(&final_calls).starts_with(&fed_to_draft),
        "the final engine must have seen the same frames, in the same order, before anything \
         the trailing drain may have added: got {:?}",
        fed_samples(&final_calls)
    );
    let draft = draft_calls.lock().expect("lock");
    assert_eq!(
        draft
            .iter()
            .filter(|call| **call == CallRecord::Finish)
            .count(),
        1,
        "the draft decoder must be finished exactly once after recording stops, got {draft:?}"
    );
    assert_eq!(draft.last(), Some(&CallRecord::Finish));
    drop(draft);

    handle.shutdown();
}

/// The draft engine is begun on the *same* `TranscribeOptions` as the final one: it is a second
/// view of a single utterance, not a differently-configured recognizer.
///
/// Both fields are deliberately given non-default values, and the final engine's recorded options
/// are checked to actually carry them, so the equality below cannot be satisfied by two
/// `TranscribeOptions::default()` meeting in the middle -- which is exactly what a
/// `begin_draft(ctx, &TranscribeOptions::default())` regression would produce. What that costs in
/// practice is named in `ProfileDeps.draft_engine`'s own doc comment: a preview begun without the
/// profile's language shows Russian speech as garbled English, which is the reason the field is
/// per-profile rather than per-runtime in the first place. Unreachable today, since nothing
/// constructs a real draft engine yet; reachable the moment Task 21 does.
#[test]
fn the_draft_engine_begins_on_the_same_options_as_the_final_engine() {
    let final_opts = Arc::new(Mutex::new(Vec::new()));
    let draft_opts = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("hello world")));
    builder.language = Some("ru".to_string());
    builder.dictionary_terms = vec!["SQLite".to_string(), "Tauri".to_string()];
    builder.begin_opts = final_opts.clone();
    builder.draft_engine = Some(draft_engine(
        "preview",
        Arc::new(Mutex::new(Vec::new())),
        draft_opts.clone(),
    ));
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    let final_begun = final_opts.lock().expect("lock");
    assert_eq!(final_begun.len(), 1);
    assert_eq!(
        final_begun[0].language.as_deref(),
        Some("ru"),
        "fixture check: the final engine must genuinely have been begun on a non-default \
         language, or the comparison below would prove nothing"
    );
    assert_eq!(
        final_begun[0].initial_prompt.as_deref(),
        Some("SQLite, Tauri"),
        "fixture check: the same for the dictionary hints"
    );
    drop(final_begun);

    assert_eq!(
        begun_with(&draft_opts),
        begun_with(&final_opts),
        "the draft engine must be begun on the same options as the final engine -- the profile's \
         language and dictionary hints included, not a default it was handed instead"
    );

    handle.shutdown();
}

/// Spec D9: the draft engine drives the preview and nothing else. The four strings here are
/// deliberately unmistakable for one another -- the draft's live partial, its flushed final
/// preview, the final engine's own partial, and the final transcript -- because a fixture where
/// any two could coincide would let a leak pass unnoticed.
///
/// The preview assertion is what keeps the rest from being vacuous: it proves the draft engine
/// really was running and its text really was in play at the moment the final transcript was
/// injected, rather than the injected text being clean because the draft never produced anything.
#[test]
fn draft_text_never_reaches_the_injected_result_or_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("history.sqlite3");
    let history = HistoryRepo::open(&db_path).expect("open history db");

    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, _notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("the accurate transcript")));
    builder.partial = Some("FINAL-ENGINE-PARTIAL".to_string());
    builder.draft_engine = Some(draft_engine(
        "DRAFT-LEAK",
        draft_calls.clone(),
        Arc::new(Mutex::new(Vec::new())),
    ));
    builder.injected = injected.clone();
    builder.history = Some(history);
    builder.capture_tx_slot = tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    let tx = capture_tx(&tx_slot);
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![100; 50],
    }))
    .expect("send frame");

    assert_eq!(
        recv_partial(&states_rx).as_deref(),
        Some("DRAFT-LEAK"),
        "a profile with a draft engine must preview *its* partial, not the final engine's"
    );

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    recv_until(&states_rx, "transcribing");
    let (phase, _, flushed_preview) = recv_partial_emission(&states_rx);
    assert_eq!(phase, "transcribing");
    assert_eq!(flushed_preview.as_deref(), Some("DRAFT-FINAL-PREVIEW"));
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(
        *injected.lock().expect("lock"),
        vec!["the accurate transcript"],
        "the injected text comes from the final engine alone"
    );

    handle.shutdown();

    let verify = HistoryRepo::open(&db_path).expect("reopen history db");
    let entries = verify.list(None, 10).expect("list history");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].raw_text, "the accurate transcript");
    assert_eq!(entries[0].final_text, "the accurate transcript");
    assert!(
        !entries[0].raw_text.contains("DRAFT") && !entries[0].final_text.contains("DRAFT"),
        "no trace of the preview may reach the history row, got {:?}",
        entries[0]
    );
}

/// A draft engine that fails mid-utterance is warned about exactly once and then dropped, and the
/// dictation it was only ever an accessory to finishes normally. The second frame is what pins
/// the drop: a runtime that merely swallowed each error would call the broken engine again (and
/// warn again) for every frame that followed, dozens per second. Both frames are waited for on
/// the *final* engine's log before the release, so both are known to have gone through the
/// recording path -- where the fan-out lives -- rather than one of them reaching the final engine
/// alone via the trailing drain, which would make the "fed exactly once" assertion pass for the
/// wrong reason.
#[test]
fn a_failing_draft_engine_does_not_break_dictation() {
    let final_calls = Arc::new(Mutex::new(Vec::new()));
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("still works")));
    builder.calls = final_calls.clone();
    builder.draft_engine = Some(Box::new(FailingSttEngine {
        fail_begin: false,
        calls: draft_calls.clone(),
    }));
    builder.injected = injected.clone();
    builder.capture_tx_slot = tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    let tx = capture_tx(&tx_slot);
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![1, 2, 3],
    }))
    .expect("send frame 1");
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![4, 5, 6],
    }))
    .expect("send frame 2");

    wait_for_feeds(&final_calls, 2);

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");

    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(
        *injected.lock().expect("lock"),
        vec!["still works"],
        "a broken preview must be invisible in the result"
    );

    assert_preview_lost_notice(&notices_rx);
    assert_no_more_notices(&notices_rx);
    assert_eq!(
        fed_samples(&draft_calls),
        vec![vec![1i16, 2, 3]],
        "the draft engine must be dropped after its first failure, not fed the frames that follow"
    );

    handle.shutdown();
}

/// A draft can decode every live frame successfully and still fail while flushing its trailing
/// context. That failure disables only future previews: the authoritative engine must still be
/// finished, injected and reported as a successful dictation.
#[test]
fn a_draft_engine_that_fails_to_finish_does_not_break_dictation() {
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("authoritative result")));
    builder.draft_engine = Some(Box::new(FakeSttEngine {
        result: Err(SttError::Engine("draft flush failed".to_string())),
        calls: draft_calls.clone(),
        partial: Some("working preview".to_string()),
        finish_delay: Duration::ZERO,
        begin_opts: Arc::new(Mutex::new(Vec::new())),
    }));
    builder.injected = injected.clone();
    builder.capture_tx_slot = tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    capture_tx(&tx_slot)
        .send(CaptureEvent::Frame(AudioFrame {
            samples: vec![7, 8, 9],
        }))
        .expect("send frame");
    wait_for_feeds(&draft_calls, 1);

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");

    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(
        *injected.lock().expect("lock"),
        vec!["authoritative result"],
        "a draft flush failure must not prevent the final result from being injected"
    );
    assert_preview_lost_notice(&notices_rx);
    assert_no_more_notices(&notices_rx);
    assert_eq!(
        *draft_calls.lock().expect("lock"),
        vec![CallRecord::Feed(vec![7, 8, 9]), CallRecord::Finish],
        "the draft must be finished once after its successful live feed"
    );

    handle.shutdown();
}

/// A preview flush is accessory work even when it never returns. Keep its
/// gate closed until the authoritative text has actually reached the
/// injector: a serial implementation deadlocks on that ordering, while the
/// bounded concurrent implementation disables the preview and injects well
/// before this test releases it.
#[test]
fn a_blocked_draft_finish_cannot_delay_authoritative_injection() {
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let capture_tx_slot = Arc::new(Mutex::new(None));
    let (finish_started_tx, finish_started_rx) = unbounded();
    let (finish_release_tx, finish_release_rx) = unbounded();
    let (finish_returned_tx, finish_returned_rx) = unbounded();
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("authoritative result")));
    builder.draft_engine = Some(Box::new(GatedFinishSttEngine {
        calls: draft_calls.clone(),
        started_tx: finish_started_tx,
        release_rx: finish_release_rx,
        returned_tx: finish_returned_tx,
    }));
    builder.injected = injected.clone();
    builder.capture_tx_slot = capture_tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    recv_until(&states_rx, "transcribing");

    let draft_started = finish_started_rx.recv_timeout(WAIT).is_ok();

    // Do not open the gate yet. The final result must cross the real
    // injector boundary while draft `finish()` is provably still blocked.
    let injection_deadline = Instant::now() + WAIT;
    let injected_before_draft_release = loop {
        if !injected.lock().expect("lock").is_empty() {
            break true;
        }
        if Instant::now() >= injection_deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(5));
    };

    // Always release the detached fixture thread before asserting, including
    // on a regression, so a failed test cannot strand a blocked thread in
    // the shared integration-test process.
    let _ = finish_release_tx.send(());
    let draft_returned = finish_returned_rx.recv_timeout(WAIT).is_ok();

    assert!(
        draft_started,
        "draft finish should have started on its worker"
    );
    assert!(
        injected_before_draft_release,
        "the authoritative result must be injected promptly while draft finish remains blocked"
    );
    assert!(draft_returned, "the released draft worker should return");
    recv_until(&states_rx, "idle");

    assert_eq!(
        *injected.lock().expect("lock"),
        vec!["authoritative result"],
        "only the final engine's result may be injected"
    );
    assert_preview_lost_notice(&notices_rx);
    assert_eq!(
        draft_calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|call| **call == CallRecord::Finish)
            .count(),
        1,
        "the timed-out draft must still be finished exactly once"
    );

    // The worker's `returned` signal comes just before it posts ownership to
    // the runtime channel. Probe through the real live-preview path instead
    // of assuming those two events are the same. If the first press wins the
    // select race, cancel that empty-preview session; its Idle boundary drains
    // the outcome, and the bounded retry observes the restored engine.
    let restore_deadline = Instant::now() + WAIT;
    loop {
        hotkey_tx
            .send(HotkeyEvent::Pressed {
                binding: BindingId::from(0),
            })
            .expect("send restored-profile pressed");
        recv_until(&states_rx, "recording");
        capture_tx(&capture_tx_slot)
            .send(CaptureEvent::Frame(AudioFrame {
                samples: vec![21, 22, 23],
            }))
            .expect("send restoration probe frame");
        if recv_partial_before(&states_rx, Duration::from_millis(100)).as_deref()
            == Some("working preview")
        {
            break;
        }
        handle.cancel();
        recv_until(&states_rx, "idle");
        assert!(
            Instant::now() < restore_deadline,
            "late healthy outcome should be processed and restore live preview within {WAIT:?}"
        );
    }

    // Keep the restored engine's second finish blocked too. This produces a
    // second real timeout while authoritative injection remains prompt, but
    // the runtime-lifetime timeout latch must suppress a duplicate notice.
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send restored-profile released");
    recv_until(&states_rx, "transcribing");
    finish_started_rx
        .recv_timeout(WAIT)
        .expect("late healthy draft should be restored for the next session");

    let second_injection_deadline = Instant::now() + WAIT;
    loop {
        if injected.lock().expect("lock").len() >= 2 {
            break;
        }
        assert!(
            Instant::now() < second_injection_deadline,
            "second authoritative result should be injected while its draft finish is blocked"
        );
        thread::sleep(Duration::from_millis(5));
    }

    let _ = finish_release_tx.send(());
    finish_returned_rx
        .recv_timeout(WAIT)
        .expect("restored draft finish should return");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(
        *injected.lock().expect("lock"),
        vec!["authoritative result", "authoritative result"]
    );
    assert_eq!(
        draft_calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|call| **call == CallRecord::Finish)
            .count(),
        2,
        "late restoration should make the draft reusable and time out a second time without reload"
    );
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

/// Once the bounded collector has reported a timeout, the same worker's
/// eventual decoder error carries no new action for the user. Its late
/// outcome is consumed, but it must not duplicate the existing notice.
#[test]
fn late_draft_failure_after_timeout_does_not_duplicate_notice() {
    let injected = Arc::new(Mutex::new(Vec::new()));
    let (draft_started_tx, draft_started_rx) = unbounded();
    let (draft_release_tx, draft_release_rx) = unbounded();
    let (draft_returned_tx, draft_returned_rx) = unbounded();
    let (draft_dropped_tx, draft_dropped_rx) = unbounded();
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("authoritative result")));
    builder.draft_engine = Some(Box::new(GatedFailingFinishSttEngine {
        calls: Arc::new(Mutex::new(Vec::new())),
        started_tx: draft_started_tx,
        release_rx: draft_release_rx,
        returned_tx: draft_returned_tx,
        dropped_tx: Some(draft_dropped_tx),
    }));
    builder.injected = injected.clone();
    let handle = Runtime::spawn(builder.build(hotkey_rx), sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    recv_until(&states_rx, "recording");
    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");
    recv_until(&states_rx, "transcribing");
    draft_started_rx
        .recv_timeout(WAIT)
        .expect("draft finish should remain blocked through timeout");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(
        *injected.lock().expect("lock"),
        vec!["authoritative result"]
    );
    assert_preview_lost_notice(&notices_rx);

    let _ = draft_release_tx.send(());
    draft_returned_rx
        .recv_timeout(WAIT)
        .expect("late failing draft should return after release");
    draft_dropped_rx
        .recv_timeout(WAIT)
        .expect("runtime should consume the late outcome and drop its failed engine");
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

/// A stuck native flush limits only background *finish workers*, not live
/// preview engines. Across a reload, the new profile must keep receiving and
/// previewing frames; its first flush is skipped while the old worker lives,
/// then the very next session can finish normally once the old outcome has
/// been discarded — no second reload required.
#[test]
fn a_stuck_draft_worker_guard_survives_reload_and_blocks_a_second_finish() {
    let first_draft_calls = Arc::new(Mutex::new(Vec::new()));
    let second_draft_calls = Arc::new(Mutex::new(Vec::new()));
    let first_injected = Arc::new(Mutex::new(Vec::new()));
    let second_injected = Arc::new(Mutex::new(Vec::new()));
    let second_capture_tx_slot = Arc::new(Mutex::new(None));

    let (first_draft_started_tx, first_draft_started_rx) = unbounded();
    let (first_draft_release_tx, first_draft_release_rx) = unbounded();
    let (first_draft_returned_tx, first_draft_returned_rx) = unbounded();
    let (first_draft_dropped_tx, first_draft_dropped_rx) = unbounded();
    let (first_final_started_tx, first_final_started_rx) = unbounded();
    let (first_final_release_tx, first_final_release_rx) = unbounded();
    let (first_final_returned_tx, first_final_returned_rx) = unbounded();
    let (second_draft_started_tx, second_draft_started_rx) = unbounded();
    let (second_draft_release_tx, second_draft_release_rx) = unbounded();
    let (second_draft_returned_tx, second_draft_returned_rx) = unbounded();
    let (sink, states_rx, notices_rx) = fake_sink();

    let (first_hotkey_tx, first_hotkey_rx) = unbounded();
    let mut first_builder = DepsBuilder::new(Ok(transcript("unused first result")));
    first_builder.engine = Some(Box::new(GatedFinishSttEngine {
        calls: Arc::new(Mutex::new(Vec::new())),
        started_tx: first_final_started_tx,
        release_rx: first_final_release_rx,
        returned_tx: first_final_returned_tx,
    }));
    first_builder.draft_engine = Some(Box::new(DropNotifyingSttEngine {
        inner: GatedFinishSttEngine {
            calls: first_draft_calls.clone(),
            started_tx: first_draft_started_tx,
            release_rx: first_draft_release_rx,
            returned_tx: first_draft_returned_tx,
        },
        dropped_tx: first_draft_dropped_tx,
    }));
    first_builder.injected = first_injected.clone();
    let handle = Runtime::spawn(first_builder.build(first_hotkey_rx), sink);

    first_hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send first pressed");
    assert_eq!(recv_state(&states_rx), "recording");
    first_hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send first released");
    assert_eq!(recv_state(&states_rx), "transcribing");
    first_draft_started_rx
        .recv_timeout(WAIT)
        .expect("first draft finish should block at its gate");
    first_final_started_rx
        .recv_timeout(WAIT)
        .expect("first final finish should block at its gate");

    handle.cancel();
    let _ = first_final_release_tx.send(());
    first_final_returned_rx
        .recv_timeout(WAIT)
        .expect("first final finish should return after release");
    recv_until(&states_rx, "idle");

    // Keep the first draft gate closed across the reload. The new profile's
    // engine must remain installed and useful for live feed even though its
    // optional flush cannot start yet.
    let (second_hotkey_tx, second_hotkey_rx) = unbounded();
    let mut second_builder = DepsBuilder::new(Ok(transcript("second result")));
    second_builder.draft_engine = Some(Box::new(GatedFinishSttEngine {
        calls: second_draft_calls.clone(),
        started_tx: second_draft_started_tx,
        release_rx: second_draft_release_rx,
        returned_tx: second_draft_returned_tx,
    }));
    second_builder.injected = second_injected.clone();
    second_builder.capture_tx_slot = second_capture_tx_slot.clone();
    handle.reload(second_builder.build(second_hotkey_rx));

    second_hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send second pressed");
    recv_until(&states_rx, "recording");
    capture_tx(&second_capture_tx_slot)
        .send(CaptureEvent::Frame(AudioFrame {
            samples: vec![11, 12, 13],
        }))
        .expect("send second-profile frame");
    assert_eq!(
        recv_partial(&states_rx).as_deref(),
        Some("working preview"),
        "a stuck old flush must not remove the new profile's live preview engine"
    );
    second_hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send second released");
    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    let second_finish_started_while_busy = second_draft_started_rx.try_recv().is_ok();
    if second_finish_started_while_busy {
        // Clean up both gates before failing so a regression cannot strand
        // background fixtures in the integration-test process.
        let _ = second_draft_release_tx.send(());
        let _ = first_draft_release_tx.send(());
        let _ = second_draft_returned_rx.recv_timeout(WAIT);
        let _ = first_draft_returned_rx.recv_timeout(WAIT);
    }
    assert!(
        !second_finish_started_while_busy,
        "the runtime must not start a second finish worker while the old one is alive"
    );

    // Release the old generation and wait until its engine is actually
    // discarded by the runtime. The worker clears its atomic permit before
    // publishing that outcome, so this handshake makes the next-session
    // assertion deterministic rather than timing-based.
    let _ = first_draft_release_tx.send(());
    first_draft_returned_rx
        .recv_timeout(WAIT)
        .expect("released old draft should return");
    first_draft_dropped_rx
        .recv_timeout(WAIT)
        .expect("old-generation outcome should be discarded after reload");

    // No further reload: the same second-profile engine survived the skipped
    // flush, begins a fresh stream, and can now acquire the worker permit.
    second_hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send third pressed");
    recv_until(&states_rx, "recording");
    second_hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send third released");
    recv_until(&states_rx, "transcribing");
    second_draft_started_rx
        .recv_timeout(WAIT)
        .expect("next session should start draft finish after old worker exits");
    let _ = second_draft_release_tx.send(());
    second_draft_returned_rx
        .recv_timeout(WAIT)
        .expect("released second-profile finish should return");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert!(
        first_injected.lock().expect("lock").is_empty(),
        "the cancelled first session must inject nothing"
    );
    assert_eq!(
        *second_injected.lock().expect("lock"),
        vec!["second result", "second result"],
        "both reloaded-profile sessions must dictate with the authoritative engine"
    );
    assert_eq!(
        second_draft_calls
            .lock()
            .expect("lock")
            .iter()
            .filter(|call| **call == CallRecord::Finish)
            .count(),
        1,
        "busy session skips flush, then the next session finishes the preserved engine exactly once"
    );
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}

/// The same policy at the other end of the draft engine's life: one that cannot even start is
/// dropped at `begin` and never fed, and the session it failed inside of proceeds as if it had
/// never been configured. The frame is waited for on the final engine's log before the release,
/// so it is known to have passed through the fan-out site itself -- a frame that only ever
/// reached the final engine via the trailing drain would leave the draft engine unfed no matter
/// what `begin` had returned.
#[test]
fn a_draft_engine_that_fails_to_begin_does_not_stop_the_session() {
    let final_calls = Arc::new(Mutex::new(Vec::new()));
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let injected = Arc::new(Mutex::new(Vec::new()));
    let tx_slot = Arc::new(Mutex::new(None));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("still works")));
    builder.calls = final_calls.clone();
    builder.draft_engine = Some(Box::new(FailingSttEngine {
        fail_begin: true,
        calls: draft_calls.clone(),
    }));
    builder.injected = injected.clone();
    builder.capture_tx_slot = tx_slot.clone();
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    let tx = capture_tx(&tx_slot);
    tx.send(CaptureEvent::Frame(AudioFrame {
        samples: vec![1, 2, 3],
    }))
    .expect("send frame");

    wait_for_feeds(&final_calls, 1);

    hotkey_tx
        .send(HotkeyEvent::Released {
            binding: BindingId::from(0),
        })
        .expect("send released");

    recv_until(&states_rx, "transcribing");
    recv_until(&states_rx, "injecting");
    recv_until(&states_rx, "idle");

    assert_eq!(*injected.lock().expect("lock"), vec!["still works"]);

    assert_preview_lost_notice(&notices_rx);
    assert_no_more_notices(&notices_rx);
    let draft = draft_calls.lock().expect("lock");
    assert!(
        draft.is_empty(),
        "a draft engine that failed to begin must never be fed, got {draft:?}"
    );
    drop(draft);

    handle.shutdown();
}

/// The ordering inside `start_capture`: the draft engine is begun *before* the final one.
///
/// The final engine's `begin` can fail, and when it does `start_capture` returns early while
/// deliberately leaving the session in `Recording` (see its comment). Begun after that point,
/// the draft engine would be skipped and yet still fanned out to by any frame already buffered
/// in the audio channel -- `feed` on an engine that was never begun, which a real
/// `SherpaStreamingEngine` answers with an internal invariant message the user would see in a
/// toast, and which costs them the preview for the rest of the run since `disable_draft` is not
/// scoped to the session.
///
/// The error notice is the synchronisation point rather than a poll: the worker emits it from
/// the very `if let Err` arm that follows `begin_draft`, so by the time it has been received the
/// draft engine has either been begun or been skipped for good -- no sleep, and no window in
/// which a wrong ordering could still pass.
#[test]
fn the_draft_engine_is_begun_even_when_the_final_engine_fails_to_begin() {
    let draft_calls = Arc::new(Mutex::new(Vec::new()));
    let draft_opts = Arc::new(Mutex::new(Vec::new()));
    let (sink, states_rx, notices_rx) = fake_sink();

    let (hotkey_tx, hotkey_rx) = unbounded();
    let mut builder = DepsBuilder::new(Ok(transcript("never reached")));
    builder.engine = Some(Box::new(FailingSttEngine {
        fail_begin: true,
        calls: Arc::new(Mutex::new(Vec::new())),
    }));
    builder.draft_engine = Some(draft_engine(
        "preview",
        draft_calls.clone(),
        draft_opts.clone(),
    ));
    let deps = builder.build(hotkey_rx);

    let handle = Runtime::spawn(deps, sink);

    hotkey_tx
        .send(HotkeyEvent::Pressed {
            binding: BindingId::from(0),
        })
        .expect("send pressed");
    assert_eq!(recv_state(&states_rx), "recording");

    let (kind, msg) = recv_notice(&notices_rx);
    assert_eq!(kind, "error");
    assert!(
        msg.contains("failed to start transcription"),
        "expected the final engine's begin failure to be reported, got {msg:?}"
    );

    assert_eq!(
        begun_with(&draft_opts).len(),
        1,
        "the draft engine must be begun before the final engine, so a final engine that fails to \
         begin cannot leave a still-Recording session fanning frames out to an un-begun draft"
    );
    assert_no_more_notices(&notices_rx);

    handle.shutdown();
}
