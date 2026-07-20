//! Booth-side observability runtime: session tracking, event forwarding,
//! and system-snapshot push.
//!
//! This module wires `booth-metrics` and the operator forwarder into the
//! main runtime loop. It owns three async tasks:
//!
//! 1. **System sampler** (spawned via [`booth_metrics::spawn_system_sampler`])
//!    — produces [`booth_hal::SystemSnapshot`] values periodically and
//!    updates Prometheus gauges.
//! 2. **Observability** — the sole subscriber that turns
//!    [`TelemetryEvent::StateTransition`] events into synthetic
//!    [`TelemetryEvent::CallStarted`] / [`TelemetryEvent::CallEnded`]
//!    markers, derives the [`CallOutcome`], buffers everything into bulk
//!    batches, and POSTs them to `/v1/events`. The forwarder is the only
//!    place where booth-side `event_id`s are stamped, which keeps
//!    idempotency simple (`{boot_id}:{seq}` is globally unique).
//! 3. **System pusher** — coalesces [`TelemetryEvent::SystemSample`] events
//!    and PUTs the latest snapshot to `/v1/system` at most once per
//!    configured interval.
//!
//! All three tasks run independently and can be disabled via
//! [`ObservabilityConfig`]. Operator HTTP calls performed by tasks in this
//! module deliberately do **not** publish `OperatorRequest` /
//! `OperatorResponse` events back onto the bus — that would create a
//! self-amplifying feedback loop where every forwarded event triggers
//! more events to forward.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use booth_hal::{CallOutcome, OperatorClient, OperatorError, TelemetryEvent};
use booth_telemetry::{TelemetryBus, TelemetryRecord};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};

use crate::event_spool::EventSpool;
use uuid::Uuid;

/// Upper bound on the best-effort final flush performed during graceful
/// shutdown. Keeps a hung network call from blocking teardown; anything not
/// acknowledged within this window is spilled to disk for replay instead.
const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// Top-level observability config block in `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservabilityConfig {
    /// Master kill switch. When `false`, no observability tasks start and
    /// the booth behaves exactly as it did before the observability PRs.
    pub enabled: bool,
    /// Stable identifier for this booth, used as the `boothId` field in
    /// every forwarded event and as the Prometheus `booth_id` label.
    pub booth_id: String,
    /// How often [`booth_metrics::SystemSampler`] runs.
    pub sample_interval_ms: u64,
    /// Forwarder-specific knobs.
    pub operator_forward: OperatorForwardConfig,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            booth_id: "booth-01".to_string(),
            sample_interval_ms: 5_000,
            operator_forward: OperatorForwardConfig::default(),
        }
    }
}

/// Tunables for the event forwarder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OperatorForwardConfig {
    /// When `false`, the forwarder is not spawned. Useful for headless dev
    /// runs that have no operator backend reachable.
    pub enabled: bool,
    /// Maximum events per `POST /v1/events` batch.
    pub batch_max: usize,
    /// Maximum delay between flushes when the buffer hasn't filled up.
    pub flush_interval_ms: u64,
    /// Hard cap on the in-memory queue size; the oldest events are dropped
    /// to make room when the operator is unreachable for too long.
    pub buffer_max: usize,
    /// Minimum interval between `PUT /v1/system` calls. The booth samples
    /// system stats at `sample_interval_ms` but only pushes at most this
    /// often.
    pub system_push_interval_ms: u64,
    /// Interval between periodic status re-pushes, in milliseconds. Ensures
    /// the operator never shows stale state even if it missed a transition
    /// push. Set to `0` to disable the heartbeat.
    pub heartbeat_interval_ms: u64,
}

impl Default for OperatorForwardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            batch_max: 200,
            flush_interval_ms: 2_000,
            buffer_max: 4_096,
            system_push_interval_ms: 5_000,
            heartbeat_interval_ms: 30_000,
        }
    }
}

/// Per-process runtime identity attached to every outgoing event.
///
/// `boot_id` is a fresh UUIDv4 minted at process start so the operator can
/// totally order events across booth reboots without trusting the booth's
/// wall clock.
#[derive(Debug, Clone)]
pub struct RuntimeIdentity {
    /// Stable booth id (matches [`ObservabilityConfig::booth_id`]).
    pub booth_id: String,
    /// UUIDv4 minted on this process start.
    pub boot_id: String,
    /// Running `telephone-booth` client version (e.g. `0.3.2`). Stamped
    /// on every event + system snapshot so operators can see which booth
    /// build is online.
    pub version: &'static str,
    /// Used to compute monotonic nanoseconds relative to process start.
    pub start: Instant,
}

impl RuntimeIdentity {
    /// Mint a new identity for the current process.
    #[must_use]
    pub fn new(booth_id: impl Into<String>) -> Self {
        Self {
            booth_id: booth_id.into(),
            boot_id: Uuid::new_v4().to_string(),
            version: env!("CARGO_PKG_VERSION"),
            start: Instant::now(),
        }
    }
}

/// Cheap clone-able handle to the active call session id.
///
/// The runtime's effect executor reads this when emitting events that
/// belong to a call (`RecordingStarted`, `UploadStarted`, …) so those
/// events can carry the session id without taking a lock per emit. The
/// observability task is the sole writer.
#[derive(Debug, Clone, Default)]
pub struct SessionHandle {
    inner: Arc<Mutex<Option<String>>>,
}

impl SessionHandle {
    /// Return the currently-active session id, if any.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        self.inner.lock().clone()
    }

    fn set(&self, id: Option<String>) {
        *self.inner.lock() = id;
    }
}

/// Internal: live state about the in-progress call.
#[derive(Debug, Clone)]
struct LiveSession {
    id: String,
    phase: SessionPhase,
    digits: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    /// Off-hook, dial tone, dialing, or playing the prompt — no recording
    /// has started yet.
    Prompt,
    /// Recording the caller's answer.
    Recording,
    /// Uploading the finished recording.
    Uploading,
    /// Upload succeeded; awaiting hangup.
    RecordingDone,
    /// Upload failed; awaiting hangup.
    RecordingFailed,
    /// Operator pre-prompt fetch failed; awaiting hangup.
    OperatorErrorPhase,
}

/// Pure state machine that derives `CallStarted` / `CallEnded` from
/// observed [`TelemetryEvent`] sequences.
#[derive(Debug, Default)]
pub struct SessionTracker {
    current: Option<LiveSession>,
}

impl SessionTracker {
    /// Create a fresh tracker with no active session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one event and return any synthetic events to forward.
    ///
    /// `monotonic_ns` is stamped onto any synthesized events so the
    /// operator can order them within the booth's process lifetime.
    pub fn observe(&mut self, event: &TelemetryEvent, monotonic_ns: u64) -> Vec<TelemetryEvent> {
        let mut out = Vec::new();
        match event {
            TelemetryEvent::StateTransition { from, to, .. } => {
                let was_idle = from == "idle";
                let now_idle = to == "idle";
                if was_idle && !now_idle && self.current.is_none() {
                    let id = Uuid::new_v4().to_string();
                    self.current = Some(LiveSession {
                        id: id.clone(),
                        phase: SessionPhase::Prompt,
                        digits: String::new(),
                    });
                    out.push(TelemetryEvent::CallStarted {
                        session_id: id,
                        at_monotonic_ns: monotonic_ns,
                    });
                }
                if let Some(session) = self.current.as_mut() {
                    match to.as_str() {
                        "recording" => session.phase = SessionPhase::Recording,
                        "uploading" => session.phase = SessionPhase::Uploading,
                        "error" => session.phase = SessionPhase::OperatorErrorPhase,
                        _ => {}
                    }
                }
                if !was_idle
                    && now_idle
                    && let Some(session) = self.current.take()
                {
                    let outcome = match session.phase {
                        SessionPhase::Prompt if session.digits.is_empty() => {
                            CallOutcome::HungUpBeforeDial
                        }
                        SessionPhase::Prompt => CallOutcome::HungUpDuringPrompt,
                        SessionPhase::Recording => CallOutcome::HungUpDuringRecording,
                        SessionPhase::Uploading => CallOutcome::HungUpDuringUpload,
                        SessionPhase::RecordingDone => CallOutcome::RecordingCompleted,
                        SessionPhase::RecordingFailed => CallOutcome::UploadFailed,
                        SessionPhase::OperatorErrorPhase => CallOutcome::OperatorError,
                    };
                    out.push(TelemetryEvent::CallEnded {
                        session_id: session.id,
                        outcome,
                        at_monotonic_ns: monotonic_ns,
                    });
                }
            }
            TelemetryEvent::DigitDialed { digit, .. } => {
                if let Some(session) = self.current.as_mut() {
                    session.digits.push_str(&digit.to_string());
                }
            }
            TelemetryEvent::UploadCompleted { .. } => {
                if let Some(session) = self.current.as_mut() {
                    session.phase = SessionPhase::RecordingDone;
                }
            }
            TelemetryEvent::UploadFailed { .. } => {
                if let Some(session) = self.current.as_mut() {
                    session.phase = SessionPhase::RecordingFailed;
                }
            }
            _ => {}
        }
        out
    }

    /// Snapshot the current session id, if any.
    #[must_use]
    pub fn current_session_id(&self) -> Option<&str> {
        self.current.as_ref().map(|s| s.id.as_str())
    }
}

/// Spawn the event forwarder task.
///
/// Returns the join handle. On graceful shutdown the caller should send
/// `true` on the `shutdown` watch channel and await the handle (with a
/// timeout) so the task gets a chance to make a best-effort final flush and
/// durably spill any still-buffered events to disk before exiting. Buffered
/// events that are never spilled would otherwise be lost across a restart —
/// including a `CallEnded` that closes a call session.
///
/// When an [`super::event_spool::EventSpool`] is provided, failed batches are persisted to disk
/// and replayed on startup so events survive restarts and extended outages.
#[allow(clippy::needless_pass_by_value)]
pub fn spawn_event_forwarder(
    bus: TelemetryBus,
    operator: Arc<dyn OperatorClient>,
    identity: RuntimeIdentity,
    config: ObservabilityConfig,
    session_handle: SessionHandle,
    event_spool: Option<Arc<EventSpool>>,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    // Subscribe synchronously so we don't miss any events that fire
    // between spawn and the task's first poll.
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        // Replay any spooled batches from a previous run before processing
        // new events. Events have stable eventIds so replay is idempotent.
        if let Some(ref spool) = event_spool {
            for (path, body) in spool.drain() {
                match operator.push_events_json(&body).await {
                    Ok(ack) => {
                        debug!(
                            accepted = ack.accepted,
                            duplicates = ack.duplicates,
                            "replayed spooled event batch"
                        );
                        EventSpool::remove_file(&path);
                    }
                    Err(OperatorError::Unsupported(_)) => {
                        EventSpool::remove_file(&path);
                    }
                    Err(err) => {
                        warn!(%err, "failed to replay spooled events; will retry next startup");
                        break;
                    }
                }
            }
        }

        let mut tracker = SessionTracker::new();
        let mut batch: VecDeque<Value> = VecDeque::with_capacity(config.operator_forward.batch_max);
        let mut dropped: u64 = 0;
        let mut flush = tokio::time::interval(Duration::from_millis(
            config.operator_forward.flush_interval_ms,
        ));
        flush.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut next_seq: u64 = 0;
        let mut was_failing = false;

        loop {
            tokio::select! {
                () = flush_tick(&mut flush) => {
                    if !batch.is_empty() {
                        // Race the flush against the shutdown signal so a
                        // hung operator call (the real client has a 10 s HTTP
                        // timeout) cannot pin the loop and delay the bounded
                        // final flush + spill below. `flush_once` is
                        // cancellation-safe: if shutdown wins, the batch is
                        // left intact for the shutdown drain to persist.
                        tokio::select! {
                            ok = flush_once(&operator, &mut batch, &identity) => {
                                after_flush(ok, &mut was_failing, event_spool.as_ref(), &mut batch, &config);
                            }
                            () = shutdown_requested(&mut shutdown) => break,
                        }
                    }
                }
                received = rx.recv() => {
                    match received {
                        Ok(record) => {
                            ingest_record(
                                &record,
                                &mut tracker,
                                &session_handle,
                                &identity,
                                &mut next_seq,
                                &mut batch,
                                &mut dropped,
                                &bus,
                                config.operator_forward.buffer_max,
                            );
                            if batch.len() >= config.operator_forward.batch_max {
                                tokio::select! {
                                    ok = flush_once(&operator, &mut batch, &identity) => {
                                        after_flush(ok, &mut was_failing, event_spool.as_ref(), &mut batch, &config);
                                    }
                                    () = shutdown_requested(&mut shutdown) => break,
                                }
                            }
                            if dropped > 0 {
                                warn!(dropped, "operator event forwarder buffer overflow");
                                dropped = 0;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!(skipped, "operator event forwarder lagged on telemetry bus");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                () = shutdown_requested(&mut shutdown) => {
                    // The value became `true` or the sender was dropped;
                    // either way we stop the loop and run the shutdown drain +
                    // final best-effort flush + spill below.
                    break;
                }
            }
        }

        // Graceful shutdown. Records may still be sitting in the broadcast
        // queue that were published by `handle_event` before the shutdown
        // signal but never pulled into `batch` (e.g. the `HookOn`
        // StateTransition that synthesizes `CallEnded`). Drain them now so the
        // final flush + spill can see them — otherwise an immediate shutdown
        // after `HookOn` would still lose the `CallEnded`.
        loop {
            match rx.try_recv() {
                Ok(record) => ingest_record(
                    &record,
                    &mut tracker,
                    &session_handle,
                    &identity,
                    &mut next_seq,
                    &mut batch,
                    &mut dropped,
                    &bus,
                    config.operator_forward.buffer_max,
                ),
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
            }
        }

        // Make a bounded best-effort flush of whatever is still buffered, then
        // durably spill anything that wasn't acknowledged so the next startup
        // replays it. Without this, a normal shutdown (SIGTERM, power-cycle
        // window, restart) would drop in-memory events — including the
        // `CallEnded` that ends a call session.
        if !batch.is_empty() {
            let flushed = matches!(
                tokio::time::timeout(
                    SHUTDOWN_FLUSH_TIMEOUT,
                    flush_once(&operator, &mut batch, &identity),
                )
                .await,
                Ok(true)
            );
            if !flushed {
                spill_remaining(event_spool.as_ref(), &mut batch);
            }
        }
    })
}

/// Spawn the system snapshot pusher task.
#[allow(clippy::needless_pass_by_value)]
pub fn spawn_system_pusher(
    bus: TelemetryBus,
    operator: Arc<dyn OperatorClient>,
    identity: RuntimeIdentity,
    config: ObservabilityConfig,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let mut last_pushed = Instant::now()
            .checked_sub(Duration::from_hours(1))
            .unwrap_or_else(Instant::now);
        let min_interval = Duration::from_millis(config.operator_forward.system_push_interval_ms);
        loop {
            match rx.recv().await {
                Ok(record) => {
                    if let TelemetryEvent::SystemSample { snapshot, .. } = record.event {
                        if last_pushed.elapsed() < min_interval {
                            continue;
                        }
                        if let Err(err) = operator
                            .put_system_snapshot(&identity.booth_id, identity.version, &snapshot)
                            .await
                        {
                            warn!(%err, "PUT /v1/system failed");
                        } else {
                            last_pushed = Instant::now();
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => (),
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Spawn the status heartbeat task.
///
/// Periodically re-pushes the booth's current `BoothStatus` so the
/// operator never shows stale state — even if it missed an earlier
/// transition push due to a transient network failure.
#[allow(clippy::needless_pass_by_value)]
pub fn spawn_status_heartbeat(
    bus: TelemetryBus,
    operator: Arc<dyn OperatorClient>,
    config: ObservabilityConfig,
) -> JoinHandle<()> {
    use booth_hal::BoothStatus;

    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let interval_ms = config.operator_forward.heartbeat_interval_ms;
        if interval_ms == 0 {
            return;
        }
        let mut heartbeat = tokio::time::interval(Duration::from_millis(interval_ms));
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut current_status: BoothStatus = BoothStatus::Idle;

        loop {
            tokio::select! {
                () = flush_tick(&mut heartbeat) => {
                    if let Err(err) = operator.put_status(current_status).await {
                        debug!(%err, "status heartbeat PUT /v1/status failed");
                    } else {
                        debug!(status = %current_status, "status heartbeat pushed");
                    }
                }
                received = rx.recv() => {
                    match received {
                        Ok(record) => {
                            if let TelemetryEvent::StateTransition { to, .. } = &record.event {
                                current_status = state_name_to_booth_status(to);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => (),
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    })
}

/// Map a state-machine state name to the coarse [`BoothStatus`] pushed to
/// the operator.
fn state_name_to_booth_status(name: &str) -> booth_hal::BoothStatus {
    use booth_hal::BoothStatus;
    match name {
        "idle" | "Idle" => BoothStatus::Idle,
        "dial_tone" | "dialing" | "DialTone" => BoothStatus::DialTone,
        "playing_question" | "beep" | "PlayingQuestion" => BoothStatus::PlayingQuestion,
        "recording" | "Recording" => BoothStatus::Recording,
        "uploading" | "Uploading" => BoothStatus::Uploading,
        "playing_message" | "PlayingMessage" => BoothStatus::PlayingMessage,
        "playing_instructions" | "PlayingInstructions" => BoothStatus::PlayingInstructions,
        "call_unavailable" | "CallUnavailable" => BoothStatus::CallUnavailable,
        // Conservative fallback: if we see an unknown state name, report Idle
        // rather than panicking. The operator treats unknown states gracefully.
        _ => BoothStatus::Idle,
    }
}

async fn flush_tick(interval: &mut tokio::time::Interval) {
    interval.tick().await;
}

/// Resolve as soon as the shutdown watch is (or becomes) `true`, or the
/// sender is dropped. Used to race in-loop operator calls against shutdown so
/// a hung request can't delay the bounded final flush + spill.
async fn shutdown_requested(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow_and_update() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

/// Ingest one telemetry record into the batch: run it through the session
/// tracker (which may synthesize `CallStarted` / `CallEnded`), stamp wire
/// envelopes, and publish any synthesized events back onto the bus.
#[allow(clippy::too_many_arguments)]
fn ingest_record(
    record: &TelemetryRecord,
    tracker: &mut SessionTracker,
    session_handle: &SessionHandle,
    identity: &RuntimeIdentity,
    next_seq: &mut u64,
    batch: &mut VecDeque<Value>,
    dropped: &mut u64,
    bus: &TelemetryBus,
    buffer_max: usize,
) {
    // Process the original event through the tracker first so synthesized
    // CallStarted/CallEnded events are stamped with the same monotonic time
    // as the event that produced them.
    let monotonic_ns = monotonic_ns_of(record);
    let synthetic = tracker.observe(&record.event, monotonic_ns);
    session_handle.set(tracker.current_session_id().map(str::to_string));

    if let Some(wire) = wire_for(record, identity, next_seq) {
        push_with_cap(batch, wire, buffer_max, dropped);
    }
    for synth in synthetic {
        let wire = wire_for_synthetic(&synth, identity, next_seq);
        push_with_cap(batch, wire, buffer_max, dropped);
        // Also publish back to the bus so other subscribers (booth-debug WS,
        // system_pusher filters, etc.) see the synthesized events.
        bus.publish(synth);
    }
}

/// Update the `was_failing` recovery flag after a flush attempt, spilling the
/// batch when the operator is unreachable so it survives buffer pressure.
fn after_flush(
    ok: bool,
    was_failing: &mut bool,
    spool: Option<&Arc<EventSpool>>,
    batch: &mut VecDeque<Value>,
    config: &ObservabilityConfig,
) {
    if ok && *was_failing {
        debug!("operator reconnected; event forwarder recovered");
        *was_failing = false;
    } else if !ok {
        *was_failing = true;
        maybe_spill(spool, batch, config);
    }
}

/// Attempt to flush the event batch to the operator.
///
/// Returns `true` on success (batch cleared), `false` on failure (batch
/// retained for next attempt).
async fn flush_once(
    operator: &Arc<dyn OperatorClient>,
    batch: &mut VecDeque<Value>,
    identity: &RuntimeIdentity,
) -> bool {
    if batch.is_empty() {
        return true;
    }
    let events: Vec<&Value> = batch.iter().collect();
    let body = json!({ "events": events }).to_string();
    match operator.push_events_json(&body).await {
        Ok(ack) => {
            debug!(
                accepted = ack.accepted,
                duplicates = ack.duplicates,
                booth = %identity.booth_id,
                "POST /v1/events flushed"
            );
            batch.clear();
            true
        }
        Err(OperatorError::Unsupported(_)) => {
            // The operator client doesn't support events at all; drop
            // the batch silently so we don't loop forever.
            batch.clear();
            true
        }
        Err(err) => {
            // Keep the batch buffered for the next flush; the buffer cap
            // in push_with_cap will eventually drop oldest if the
            // operator stays unreachable.
            warn!(%err, "POST /v1/events failed; keeping batch buffered");
            false
        }
    }
}

fn push_with_cap(batch: &mut VecDeque<Value>, item: Value, cap: usize, dropped_counter: &mut u64) {
    if batch.len() >= cap {
        // Drop the oldest entry so newer events survive. VecDeque does
        // this in O(1) which matters when the operator is unreachable
        // for an extended period and the buffer stays at capacity.
        batch.pop_front();
        *dropped_counter = dropped_counter.saturating_add(1);
    }
    batch.push_back(item);
}

/// Spill the in-memory batch to disk when the operator is unreachable and
/// the buffer is approaching capacity. This prevents event loss across
/// extended outages or booth restarts.
fn maybe_spill(
    spool: Option<&Arc<EventSpool>>,
    batch: &mut VecDeque<Value>,
    config: &ObservabilityConfig,
) {
    let Some(spool) = spool else { return };
    // Only spill when the buffer is at least half full — avoids spilling
    // tiny batches on every transient hiccup.
    if batch.len() < config.operator_forward.buffer_max / 2 {
        return;
    }
    let items: Vec<Value> = batch.drain(..).collect();
    if let Err(err) = spool.spill(&items) {
        warn!(%err, "failed to spill event batch to disk");
        // Put the items back so push_with_cap can still evict oldest.
        for item in items {
            batch.push_back(item);
        }
    } else {
        debug!(count = items.len(), "spilled event batch to disk");
    }
}

/// Unconditionally spill every buffered event to disk (used on shutdown).
///
/// Unlike [`maybe_spill`], this does not wait for the buffer to fill up: at
/// shutdown even a single un-acknowledged event (e.g. a `CallEnded`) must be
/// persisted so the next startup replays it.
fn spill_remaining(spool: Option<&Arc<EventSpool>>, batch: &mut VecDeque<Value>) {
    if batch.is_empty() {
        return;
    }
    let Some(spool) = spool else {
        warn!(
            count = batch.len(),
            "no event spool configured; dropping buffered events on shutdown"
        );
        batch.clear();
        return;
    };
    let items: Vec<Value> = batch.drain(..).collect();
    if let Err(err) = spool.spill(&items) {
        warn!(%err, count = items.len(), "failed to spill buffered events on shutdown");
    } else {
        debug!(
            count = items.len(),
            "spilled buffered events to disk on shutdown"
        );
    }
}

fn monotonic_ns_of(record: &TelemetryRecord) -> u64 {
    match &record.event {
        TelemetryEvent::StateTransition {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::DigitDialed {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::SystemSample {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::CallStarted {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::CallEnded {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::RecordingStarted {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::RecordingStopped {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::UploadStarted {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::UploadCompleted {
            at_monotonic_ns, ..
        }
        | TelemetryEvent::UploadFailed {
            at_monotonic_ns, ..
        } => *at_monotonic_ns,
        _ => 0,
    }
}

/// Build a wire envelope for a telemetry record, or return `None` to skip
/// the event (e.g. very high-volume meter samples, or feedback-loop events
/// emitted by the forwarder itself).
fn wire_for(
    record: &TelemetryRecord,
    identity: &RuntimeIdentity,
    next_seq: &mut u64,
) -> Option<Value> {
    if !should_forward(&record.event) {
        return None;
    }
    let kind = event_kind(&record.event);
    let session_id = session_id_of(&record.event);
    let recording_id = recording_id_of(&record.event);
    let payload = event_payload(&record.event);
    let seq = next_event_seq(next_seq);
    Some(envelope(
        identity,
        kind,
        session_id,
        recording_id,
        payload,
        seq,
    ))
}

fn wire_for_synthetic(
    event: &TelemetryEvent,
    identity: &RuntimeIdentity,
    next_seq: &mut u64,
) -> Value {
    let kind = event_kind(event);
    let session_id = session_id_of(event);
    let recording_id = recording_id_of(event);
    let payload = event_payload(event);
    let seq = next_event_seq(next_seq);
    envelope(identity, kind, session_id, recording_id, payload, seq)
}

fn next_event_seq(next_seq: &mut u64) -> u64 {
    let s = *next_seq;
    *next_seq = next_seq.saturating_add(1);
    s
}

fn envelope(
    identity: &RuntimeIdentity,
    kind: &str,
    session_id: Option<String>,
    recording_id: Option<String>,
    payload: Value,
    seq: u64,
) -> Value {
    let event_id = format!("{}:{seq}", identity.boot_id);
    let occurred_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::new());
    let mut map = serde_json::Map::with_capacity(9);
    map.insert("eventId".to_string(), Value::String(event_id));
    map.insert(
        "boothId".to_string(),
        Value::String(identity.booth_id.clone()),
    );
    map.insert(
        "bootId".to_string(),
        Value::String(identity.boot_id.clone()),
    );
    map.insert("type".to_string(), Value::String(kind.to_string()));
    map.insert("occurredAt".to_string(), Value::String(occurred_at));
    map.insert(
        "version".to_string(),
        Value::String(identity.version.to_string()),
    );
    if let Some(id) = session_id {
        map.insert("sessionId".to_string(), Value::String(id));
    }
    if let Some(id) = recording_id {
        map.insert("recordingId".to_string(), Value::String(id));
    }
    map.insert("payload".to_string(), payload);
    Value::Object(map)
}

fn should_forward(event: &TelemetryEvent) -> bool {
    match event {
        // Skip high-rate meter / GPIO events that would flood the
        // operator. They're already covered by Prometheus metrics.
        TelemetryEvent::AudioLevel(_) | TelemetryEvent::GpioEdge(_) => false,
        // System samples have their own dedicated PUT /v1/system route.
        TelemetryEvent::SystemSample { .. } => false,
        // Synthetic call markers are already forwarded directly via
        // wire_for_synthetic when they are produced by the SessionTracker.
        // Suppress them here so they are not forwarded a second time when
        // republished to the bus.
        TelemetryEvent::CallStarted { .. } | TelemetryEvent::CallEnded { .. } => false,
        _ => true,
    }
}

fn event_kind(event: &TelemetryEvent) -> &'static str {
    match event {
        TelemetryEvent::GpioEdge(_) => "gpio_edge",
        TelemetryEvent::DigitDialed { .. } => "digit_dialed",
        TelemetryEvent::StateTransition { .. } => "state_transition",
        TelemetryEvent::AudioLevel(_) => "audio_level",
        TelemetryEvent::AudioDeviceChange { .. } => "audio_device_change",
        TelemetryEvent::OperatorRequest { .. } => "operator_request",
        TelemetryEvent::OperatorResponse { .. } => "operator_response",
        TelemetryEvent::Log { .. } => "log",
        TelemetryEvent::Error { .. } => "error",
        TelemetryEvent::SystemSample { .. } => "system_sample",
        TelemetryEvent::CallStarted { .. } => "call_started",
        TelemetryEvent::CallEnded { .. } => "call_ended",
        TelemetryEvent::RecordingStarted { .. } => "recording_started",
        TelemetryEvent::RecordingStopped { .. } => "recording_stopped",
        TelemetryEvent::UploadStarted { .. } => "upload_started",
        TelemetryEvent::UploadCompleted { .. } => "upload_completed",
        TelemetryEvent::UploadFailed { .. } => "upload_failed",
    }
}

fn session_id_of(event: &TelemetryEvent) -> Option<String> {
    match event {
        TelemetryEvent::CallStarted { session_id, .. }
        | TelemetryEvent::CallEnded { session_id, .. }
        | TelemetryEvent::RecordingStarted { session_id, .. }
        | TelemetryEvent::RecordingStopped { session_id, .. }
        | TelemetryEvent::UploadStarted { session_id, .. }
        | TelemetryEvent::UploadCompleted { session_id, .. }
        | TelemetryEvent::UploadFailed { session_id, .. } => Some(session_id.clone()),
        _ => None,
    }
}

fn recording_id_of(event: &TelemetryEvent) -> Option<String> {
    match event {
        TelemetryEvent::RecordingStarted { id, .. }
        | TelemetryEvent::RecordingStopped { id, .. } => Some(id.clone()),
        TelemetryEvent::UploadStarted { recording_id, .. }
        | TelemetryEvent::UploadCompleted { recording_id, .. }
        | TelemetryEvent::UploadFailed { recording_id, .. } => Some(recording_id.clone()),
        _ => None,
    }
}

fn event_payload(event: &TelemetryEvent) -> Value {
    serde_json::to_value(event).unwrap_or(Value::Null)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests may panic on setup failure"
)]
mod tests {
    use super::*;
    use booth_hal::{AudioChannel, AudioLevel};

    fn transition(from: &str, to: &str, monotonic_ns: u64) -> TelemetryEvent {
        TelemetryEvent::StateTransition {
            from: from.to_string(),
            to: to.to_string(),
            cause: "test".to_string(),
            at_monotonic_ns: monotonic_ns,
        }
    }

    #[test]
    fn pickup_emits_call_started() {
        let mut t = SessionTracker::new();
        let out = t.observe(&transition("idle", "dial_tone", 1), 1);
        assert!(matches!(
            out.as_slice(),
            [TelemetryEvent::CallStarted { .. }]
        ));
        assert!(t.current_session_id().is_some());
    }

    #[test]
    fn hangup_without_dial_emits_hung_up_before_dial() {
        let mut t = SessionTracker::new();
        t.observe(&transition("idle", "dial_tone", 1), 1);
        let out = t.observe(&transition("dial_tone", "idle", 2), 2);
        assert!(matches!(
            out.as_slice(),
            [TelemetryEvent::CallEnded {
                outcome: CallOutcome::HungUpBeforeDial,
                ..
            }]
        ));
        assert!(t.current_session_id().is_none());
    }

    #[test]
    fn hangup_after_digit_dialed_emits_hung_up_during_prompt() {
        let mut t = SessionTracker::new();
        t.observe(&transition("idle", "dial_tone", 1), 1);
        t.observe(
            &TelemetryEvent::DigitDialed {
                digit: 1,
                pulses: 1,
                at_monotonic_ns: 2,
            },
            2,
        );
        let out = t.observe(&transition("playing_question", "idle", 3), 3);
        assert!(matches!(
            out.as_slice(),
            [TelemetryEvent::CallEnded {
                outcome: CallOutcome::HungUpDuringPrompt,
                ..
            }]
        ));
    }

    #[test]
    fn completed_upload_then_idle_emits_recording_completed() {
        let mut t = SessionTracker::new();
        t.observe(&transition("idle", "dial_tone", 1), 1);
        t.observe(&transition("dial_tone", "recording", 2), 2);
        t.observe(&transition("recording", "uploading", 3), 3);
        t.observe(
            &TelemetryEvent::UploadCompleted {
                recording_id: "rec-1".to_string(),
                session_id: "sess-1".to_string(),
                duration_ms: 100,
                bytes: 1024,
                at_monotonic_ns: 4,
            },
            4,
        );
        let out = t.observe(&transition("playing_message", "idle", 5), 5);
        assert!(matches!(
            out.as_slice(),
            [TelemetryEvent::CallEnded {
                outcome: CallOutcome::RecordingCompleted,
                ..
            }]
        ));
    }

    #[test]
    fn audio_level_event_is_skipped_by_forwarder() {
        let evt = TelemetryEvent::AudioLevel(AudioLevel {
            channel: AudioChannel::Input,
            peak: 0.1,
            rms: 0.05,
            at_monotonic_ns: 0,
        });
        assert!(!should_forward(&evt));
    }

    #[test]
    fn synthetic_call_events_are_skipped_by_forwarder() {
        let started = TelemetryEvent::CallStarted {
            session_id: "sess-1".to_string(),
            at_monotonic_ns: 1,
        };
        let ended = TelemetryEvent::CallEnded {
            session_id: "sess-1".to_string(),
            outcome: CallOutcome::HungUpBeforeDial,
            at_monotonic_ns: 2,
        };
        assert!(!should_forward(&started));
        assert!(!should_forward(&ended));
    }

    #[test]
    fn envelope_uses_boot_id_for_event_id() {
        let identity = RuntimeIdentity::new("booth-test");
        let mut seq = 0;
        let env = envelope(
            &identity,
            "call_started",
            Some("sess-1".to_string()),
            None,
            Value::Null,
            next_event_seq(&mut seq),
        );
        let event_id = env.get("eventId").and_then(|v| v.as_str()).unwrap();
        assert!(event_id.starts_with(&identity.boot_id));
        assert!(event_id.ends_with(":0"));
    }

    #[test]
    fn envelope_includes_client_version() {
        let identity = RuntimeIdentity::new("booth-test");
        let mut seq = 0;
        let env = envelope(
            &identity,
            "call_started",
            Some("sess-1".to_string()),
            None,
            Value::Null,
            next_event_seq(&mut seq),
        );
        let version = env.get("version").and_then(|v| v.as_str()).unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        assert_eq!(version, identity.version);
    }
}
