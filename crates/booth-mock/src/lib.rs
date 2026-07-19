//! Mock implementations of every [`booth_hal`] trait, for tests and dev runs.
//!
//! These adapters never touch real hardware: GPIO edges come from an in-memory
//! channel, audio "playback" is just a future that resolves immediately or on
//! command, and the [`MockOperatorClient`] returns canned responses with
//! configurable failure injection.
//!
//! The mocks are deliberately verbose and easy-to-read — they are also the
//! reference implementation that future no_std adapters can model themselves
//! after.

#![warn(missing_docs)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use booth_hal::{
    AudioChannel, AudioError, AudioLevel, AudioRef, AudioSink, AudioSource, BoothStatus,
    EventBatchAck, GpioEdge, GpioError, GpioPort, OperatorClient, OperatorError, OperatorMessage,
    OperatorQuestion, PinRole, RecordingId, Storage, StorageError, SystemSnapshot, TelemetryEvent,
    UploadSlot,
};
use booth_telemetry::TelemetryBus;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, mpsc};

// ---------------------------------------------------------------------------
// GPIO
// ---------------------------------------------------------------------------

/// Sender side for synthesizing GPIO edges into the [`MockGpioPort`].
#[derive(Clone)]
pub struct GpioInjector {
    tx: mpsc::Sender<GpioEdge>,
    telemetry: Option<TelemetryBus>,
}

impl GpioInjector {
    /// Push a debounced edge into the mock GPIO stream.
    pub async fn push(&self, edge: GpioEdge) {
        if self.tx.send(edge).await.is_ok()
            && let Some(bus) = &self.telemetry
        {
            bus.publish(TelemetryEvent::GpioEdge(edge));
        }
    }
}

/// In-memory GPIO port. Pair with [`GpioInjector`] for test setup.
pub struct MockGpioPort {
    rx: mpsc::Receiver<GpioEdge>,
}

impl MockGpioPort {
    /// Create a new mock port and its injector handle.
    #[must_use]
    pub fn new() -> (Self, GpioInjector) {
        Self::build(None)
    }

    /// Create a new mock port that publishes injected edges to `bus`.
    #[must_use]
    pub fn with_telemetry(bus: &TelemetryBus) -> (Self, GpioInjector) {
        Self::build(Some(bus.clone()))
    }

    fn build(telemetry: Option<TelemetryBus>) -> (Self, GpioInjector) {
        let (tx, rx) = mpsc::channel(64);
        (Self { rx }, GpioInjector { tx, telemetry })
    }
}

#[async_trait]
impl GpioPort for MockGpioPort {
    async fn next_edge(&mut self) -> Result<GpioEdge, GpioError> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| GpioError::Stream("mock gpio channel closed".into()))
    }

    async fn snapshot(&self, _role: PinRole) -> Result<bool, GpioError> {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// A scripted audio sink that completes playback when `finish_playback` is
/// called. Useful for driving the state machine deterministically in tests.
#[derive(Default, Clone)]
pub struct MockAudioSink {
    inner: Arc<MockSinkInner>,
    telemetry: Option<TelemetryBus>,
}

#[derive(Default)]
struct MockSinkInner {
    state: Mutex<MockSinkState>,
    end: Notify,
}

/// Inspectable state of the mock audio sink.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct MockSinkState {
    /// What is currently playing (if anything).
    pub playing: Option<AudioRef>,
    /// History of every play call, oldest-first.
    pub history: Vec<AudioRef>,
}

impl MockAudioSink {
    /// Create a mock sink without telemetry publishing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock sink that publishes playback lifecycle logs to `bus`.
    #[must_use]
    pub fn with_telemetry(bus: &TelemetryBus) -> Self {
        Self {
            inner: Arc::new(MockSinkInner::default()),
            telemetry: Some(bus.clone()),
        }
    }

    /// Inspect what the sink has played.
    pub async fn state(&self) -> MockSinkState {
        self.inner.state.lock().await.clone()
    }

    /// Signal that the currently-playing source finished naturally.
    pub fn finish_playback(&self) {
        self.inner.end.notify_waiters();
        self.publish_log("playback finished".to_string());
    }

    fn publish_log(&self, message: String) {
        if let Some(bus) = &self.telemetry {
            bus.publish(TelemetryEvent::Log {
                level: "debug".to_string(),
                target: "booth_mock::audio_sink".to_string(),
                message,
            });
        }
    }
}

#[async_trait]
impl AudioSink for MockAudioSink {
    async fn play(&mut self, source: AudioRef) -> Result<(), AudioError> {
        let message = format!("play {source:?}");
        let mut s = self.inner.state.lock().await;
        s.history.push(source.clone());
        s.playing = Some(source);
        drop(s);
        self.publish_log(message);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), AudioError> {
        self.inner.state.lock().await.playing = None;
        self.publish_log("stop playback".to_string());
        Ok(())
    }

    async fn wait_for_end(&mut self) -> Result<(), AudioError> {
        let playing = self.inner.state.lock().await.playing.is_some();
        if !playing {
            return Ok(());
        }
        self.inner.end.notified().await;
        self.inner.state.lock().await.playing = None;
        self.publish_log("playback ended".to_string());
        Ok(())
    }
}

/// A scripted audio source: `start` returns an ascending recording id and
/// `stop` returns the most recent one.
#[derive(Default, Clone)]
pub struct MockAudioSource {
    inner: Arc<Mutex<MockSourceState>>,
    telemetry: Option<TelemetryBus>,
}

#[derive(Default, Debug)]
struct MockSourceState {
    next_id: u64,
    in_flight: Option<RecordingId>,
    last_finished: Option<RecordingId>,
}

impl MockAudioSource {
    /// Create a mock source without telemetry publishing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock source that publishes recording lifecycle logs to `bus`.
    #[must_use]
    pub fn with_telemetry(bus: &TelemetryBus) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockSourceState::default())),
            telemetry: Some(bus.clone()),
        }
    }

    fn publish_log(&self, message: String) {
        if let Some(bus) = &self.telemetry {
            bus.publish(TelemetryEvent::Log {
                level: "debug".to_string(),
                target: "booth_mock::audio_source".to_string(),
                message,
            });
        }
    }
}

#[async_trait]
impl AudioSource for MockAudioSource {
    async fn start(&mut self) -> Result<RecordingId, AudioError> {
        let id = {
            let mut s = self.inner.lock().await;
            s.next_id += 1;
            let id = format!("rec-{:06}", s.next_id);
            s.in_flight = Some(id.clone());
            id
        };
        self.publish_log(format!("recording started {id}"));
        Ok(id)
    }

    async fn stop(&mut self) -> Result<Option<RecordingId>, AudioError> {
        let id = {
            let mut s = self.inner.lock().await;
            let id = s.in_flight.take();
            s.last_finished.clone_from(&id);
            id
        };
        self.publish_log(format!("recording stopped {id:?}"));
        Ok(id)
    }

    async fn path_of(&self, id: &RecordingId) -> Result<String, AudioError> {
        Ok(format!("target/mock-recordings/{id}.flac"))
    }

    async fn duration_of(&self, _id: &RecordingId) -> Option<u64> {
        Some(5_000)
    }
}

/// Convenience: synthesize a periodic [`AudioLevel`] stream for the debug UI.
#[must_use]
pub fn fake_level(peak: f32, rms: f32) -> AudioLevel {
    AudioLevel {
        channel: AudioChannel::Input,
        peak,
        rms,
        at_monotonic_ns: 0,
    }
}

/// Synthesize an [`AudioLevel`] sample and publish it to `bus`.
#[must_use]
pub fn fake_level_with_telemetry(bus: &TelemetryBus, peak: f32, rms: f32) -> AudioLevel {
    let level = fake_level(peak, rms);
    bus.publish(TelemetryEvent::AudioLevel(level));
    level
}

// ---------------------------------------------------------------------------
// Operator
// ---------------------------------------------------------------------------

/// Canned, predictable operator client for tests.
#[derive(Clone)]
pub struct MockOperatorClient {
    inner: Arc<Mutex<MockOperatorState>>,
    telemetry: Option<TelemetryBus>,
    request_seq: Arc<AtomicU64>,
}

impl Default for MockOperatorClient {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockOperatorState::default())),
            telemetry: None,
            request_seq: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// In-memory state of the mock operator.
#[derive(Default)]
pub struct MockOperatorState {
    /// Pre-canned questions, popped FIFO.
    pub questions: VecDeque<OperatorQuestion>,
    /// Pre-canned messages, popped FIFO.
    pub messages: VecDeque<OperatorMessage>,
    /// Pre-canned instructions clips, popped FIFO.
    pub instructions: VecDeque<OperatorMessage>,
    /// Status writes received from the booth.
    pub statuses: Vec<BoothStatus>,
    /// Upload slots issued.
    pub uploads: Vec<UploadSlot>,
    /// If set, `random_question` will fail with this until cleared.
    pub fail_questions: Option<OperatorError>,
    /// If set, `push_events_json` will fail with this until cleared,
    /// simulating a transient API/network outage.
    pub fail_events: Option<OperatorError>,
    /// Raw `/v1/events` batch bodies received, in order of arrival.
    pub event_batches: Vec<String>,
    /// Live system snapshots received, with their booth_id label.
    pub system_snapshots: Vec<(String, String, SystemSnapshot)>,
    /// Artificial latency injected before each operator response.
    /// Useful for testing that slow network calls don't block critical effects.
    pub latency: Option<Duration>,
}

impl MockOperatorClient {
    /// Create a mock operator client without telemetry publishing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock operator client that publishes request lifecycle events to `bus`.
    #[must_use]
    pub fn with_telemetry(bus: &TelemetryBus) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockOperatorState::default())),
            telemetry: Some(bus.clone()),
            request_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Read-only access to the inner state (for assertions).
    pub fn state(&self) -> Arc<Mutex<MockOperatorState>> {
        Arc::clone(&self.inner)
    }

    fn begin_request(&self, route: &str) -> (String, Instant) {
        let request_id = format!(
            "mock-{}",
            self.request_seq.fetch_add(1, Ordering::Relaxed) + 1
        );
        if let Some(bus) = &self.telemetry {
            bus.publish(TelemetryEvent::OperatorRequest {
                id: request_id.clone(),
                route: route.to_string(),
            });
        }
        (request_id, Instant::now())
    }

    async fn apply_latency(&self) {
        let latency = self.inner.lock().await.latency;
        if let Some(d) = latency {
            tokio::time::sleep(d).await;
        }
    }

    fn finish_request<T>(
        &self,
        request_id: &str,
        started: Instant,
        result: &Result<T, OperatorError>,
    ) {
        if let Some(bus) = &self.telemetry {
            bus.publish(TelemetryEvent::OperatorResponse {
                id: request_id.to_string(),
                status: status_of(result),
                duration_ms: elapsed_ms(started),
            });
            if let Err(err) = result {
                bus.publish(TelemetryEvent::Error {
                    source: "booth_mock::operator".to_string(),
                    message: err.to_string(),
                });
            }
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn status_of<T>(result: &Result<T, OperatorError>) -> u16 {
    match result {
        Ok(_) => 200,
        Err(OperatorError::Auth(_) | OperatorError::Unauthorized(_)) => 401,
        Err(OperatorError::DuplicateRecording(_) | OperatorError::Conflict(_)) => 409,
        Err(OperatorError::InvalidArgument(_) | OperatorError::Unprocessable(_)) => 422,
        Err(OperatorError::PayloadTooLarge { .. }) => 413,
        Err(OperatorError::Server { status, .. }) => *status,
        Err(OperatorError::Protocol(_)) => 502,
        Err(OperatorError::Transport(_) | OperatorError::Unsupported(_)) => 503,
    }
}

#[async_trait]
impl OperatorClient for MockOperatorClient {
    async fn random_question(&self) -> Result<OperatorQuestion, OperatorError> {
        let (request_id, started) = self.begin_request("GET /mock/random-question");
        self.apply_latency().await;
        let result = {
            let mut s = self.inner.lock().await;
            s.fail_questions.take().map_or_else(
                || {
                    s.questions
                        .pop_front()
                        .ok_or_else(|| OperatorError::Protocol("no questions queued".into()))
                },
                Err,
            )
        };
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn random_message(&self) -> Result<OperatorMessage, OperatorError> {
        let (request_id, started) = self.begin_request("GET /mock/random-message");
        self.apply_latency().await;
        let result = {
            let mut s = self.inner.lock().await;
            s.messages
                .pop_front()
                .ok_or_else(|| OperatorError::Protocol("no messages queued".into()))
        };
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn instructions(&self) -> Result<OperatorMessage, OperatorError> {
        let (request_id, started) = self.begin_request("GET /mock/instructions");
        self.apply_latency().await;
        let result = {
            let mut s = self.inner.lock().await;
            s.instructions
                .pop_front()
                .ok_or_else(|| OperatorError::Protocol("no instructions queued".into()))
        };
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn init_upload(
        &self,
        _question_id: Option<&booth_hal::QuestionId>,
        _metadata: &booth_hal::UploadMetadata,
    ) -> Result<UploadSlot, OperatorError> {
        let (request_id, started) = self.begin_request("POST /mock/messages");
        self.apply_latency().await;
        let result = {
            let mut s = self.inner.lock().await;
            let slot_id = format!("slot-{}", s.uploads.len() + 1);
            let slot: UploadSlot = serde_json::from_value(serde_json::json!({
                "slot_id": slot_id.clone(),
                "put_url": "https://mock.invalid/upload",
                "headers": [],
                "id": slot_id,
                "uploadUrl": "https://mock.invalid/upload",
                "blobName": format!("recordings/{slot_id}.flac")
            }))
            .map_err(|err| OperatorError::Protocol(format!("mock upload slot: {err}").into()))?;
            s.uploads.push(slot.clone());
            Ok(slot)
        };
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn put_upload(
        &self,
        _slot: &UploadSlot,
        _local_path: &str,
        _sha256_hex: &str,
    ) -> Result<(), OperatorError> {
        let (request_id, started) = self.begin_request("PUT /mock/upload");
        self.apply_latency().await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        let result = Ok(());
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn complete_upload(
        &self,
        _slot_id: &str,
        _sha256_hex: &str,
        _duration_ms: u64,
    ) -> Result<(), OperatorError> {
        let (request_id, started) = self.begin_request("POST /mock/messages/complete");
        let result = Ok(());
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn put_status(&self, status: BoothStatus) -> Result<(), OperatorError> {
        let (request_id, started) = self.begin_request("PUT /mock/status");
        self.inner.lock().await.statuses.push(status);
        let result = Ok(());
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn push_events_json(&self, body: &str) -> Result<EventBatchAck, OperatorError> {
        let (request_id, started) = self.begin_request("POST /mock/events");
        // Simulate a transient API/network failure when configured. The batch
        // body is intentionally NOT recorded so tests can assert that failed
        // events are retried/spooled rather than acknowledged.
        let injected_failure = self.inner.lock().await.fail_events.clone();
        if let Some(err) = injected_failure {
            let result = Err(err);
            self.finish_request(&request_id, started, &result);
            return result;
        }
        self.inner.lock().await.event_batches.push(body.to_string());
        // Count events by a naive scan for the `"eventId"` discriminator.
        // The mock does not enforce idempotency; tests inspecting
        // `event_batches` see every retry.
        let accepted = u32::try_from(body.matches("\"eventId\"").count()).unwrap_or(u32::MAX);
        let result = Ok(EventBatchAck {
            accepted,
            duplicates: 0,
        });
        self.finish_request(&request_id, started, &result);
        result
    }

    async fn put_system_snapshot(
        &self,
        booth_id: &str,
        version: &str,
        snapshot: &SystemSnapshot,
    ) -> Result<(), OperatorError> {
        let (request_id, started) = self.begin_request("PUT /mock/system");
        self.inner.lock().await.system_snapshots.push((
            booth_id.to_string(),
            version.to_string(),
            snapshot.clone(),
        ));
        let result = Ok(());
        self.finish_request(&request_id, started, &result);
        result
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// In-memory KV storage useful for tests.
#[derive(Default, Clone)]
pub struct MockStorage {
    inner: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

#[async_trait]
impl Storage for MockStorage {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.inner.lock().await.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        self.inner
            .lock()
            .await
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.inner.lock().await.remove(key);
        Ok(())
    }
}
