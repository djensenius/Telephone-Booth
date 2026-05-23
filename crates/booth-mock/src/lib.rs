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
use std::time::Duration;

use async_trait::async_trait;
use booth_hal::{
    AudioError, AudioLevel, AudioRef, AudioSink, AudioSource, BoothStatus, GpioEdge, GpioError,
    GpioPort, OperatorClient, OperatorError, OperatorMessage, OperatorQuestion, PinRole,
    RecordingId, Storage, StorageError, UploadSlot,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, mpsc};

// ---------------------------------------------------------------------------
// GPIO
// ---------------------------------------------------------------------------

/// Sender side for synthesizing GPIO edges into the [`MockGpioPort`].
#[derive(Clone)]
pub struct GpioInjector(mpsc::Sender<GpioEdge>);

impl GpioInjector {
    /// Push a debounced edge into the mock GPIO stream.
    pub async fn push(&self, edge: GpioEdge) {
        let _ = self.0.send(edge).await;
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
        let (tx, rx) = mpsc::channel(64);
        (Self { rx }, GpioInjector(tx))
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
}

#[derive(Default)]
struct MockSinkInner {
    state: Mutex<MockSinkState>,
    end: Notify,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
/// Inspectable state of the mock audio sink.
pub struct MockSinkState {
    /// What is currently playing (if anything).
    pub playing: Option<AudioRef>,
    /// History of every play call, oldest-first.
    pub history: Vec<AudioRef>,
}

impl MockAudioSink {
    /// Inspect what the sink has played.
    pub async fn state(&self) -> MockSinkState {
        self.inner.state.lock().await.clone()
    }

    /// Signal that the currently-playing source finished naturally.
    pub fn finish_playback(&self) {
        self.inner.end.notify_waiters();
    }
}

#[async_trait]
impl AudioSink for MockAudioSink {
    async fn play(&mut self, source: AudioRef) -> Result<(), AudioError> {
        let mut s = self.inner.state.lock().await;
        s.history.push(source.clone());
        s.playing = Some(source);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), AudioError> {
        self.inner.state.lock().await.playing = None;
        Ok(())
    }

    async fn wait_for_end(&mut self) -> Result<(), AudioError> {
        let playing = self.inner.state.lock().await.playing.is_some();
        if !playing {
            return Ok(());
        }
        self.inner.end.notified().await;
        self.inner.state.lock().await.playing = None;
        Ok(())
    }
}

/// A scripted audio source: `start` returns an ascending recording id and
/// `stop` returns the most recent one.
#[derive(Default, Clone)]
pub struct MockAudioSource {
    inner: Arc<Mutex<MockSourceState>>,
}

#[derive(Default, Debug)]
struct MockSourceState {
    next_id: u64,
    in_flight: Option<RecordingId>,
    last_finished: Option<RecordingId>,
}

#[async_trait]
impl AudioSource for MockAudioSource {
    async fn start(&mut self) -> Result<RecordingId, AudioError> {
        let mut s = self.inner.lock().await;
        s.next_id += 1;
        let id = format!("rec-{:06}", s.next_id);
        s.in_flight = Some(id.clone());
        Ok(id)
    }

    async fn stop(&mut self) -> Result<Option<RecordingId>, AudioError> {
        let mut s = self.inner.lock().await;
        let id = s.in_flight.take();
        s.last_finished = id.clone();
        Ok(id)
    }

    async fn path_of(&self, id: &RecordingId) -> Result<String, AudioError> {
        Ok(format!("/tmp/mock/{id}.flac"))
    }
}

/// Convenience: synthesize a periodic [`AudioLevel`] stream for the debug UI.
#[must_use]
pub fn fake_level(peak: f32, rms: f32) -> AudioLevel {
    AudioLevel {
        channel: booth_hal::AudioChannel::Input,
        peak,
        rms,
        at_monotonic_ns: 0,
    }
}

// ---------------------------------------------------------------------------
// Operator
// ---------------------------------------------------------------------------

/// Canned, predictable operator client for tests.
#[derive(Clone, Default)]
pub struct MockOperatorClient {
    inner: Arc<Mutex<MockOperatorState>>,
}

/// In-memory state of the mock operator.
#[derive(Default)]
pub struct MockOperatorState {
    /// Pre-canned questions, popped FIFO.
    pub questions: VecDeque<OperatorQuestion>,
    /// Pre-canned messages, popped FIFO.
    pub messages: VecDeque<OperatorMessage>,
    /// Status writes received from the booth.
    pub statuses: Vec<BoothStatus>,
    /// Upload slots issued.
    pub uploads: Vec<UploadSlot>,
    /// If set, `random_question` will fail with this until cleared.
    pub fail_questions: Option<OperatorError>,
}

impl MockOperatorClient {
    /// Read-only access to the inner state (for assertions).
    pub fn state(&self) -> Arc<Mutex<MockOperatorState>> {
        Arc::clone(&self.inner)
    }
}

#[async_trait]
impl OperatorClient for MockOperatorClient {
    async fn random_question(&self) -> Result<OperatorQuestion, OperatorError> {
        let mut s = self.inner.lock().await;
        if let Some(err) = s.fail_questions.take() {
            return Err(err);
        }
        s.questions
            .pop_front()
            .ok_or_else(|| OperatorError::Protocol("no questions queued".into()))
    }

    async fn random_message(&self) -> Result<OperatorMessage, OperatorError> {
        let mut s = self.inner.lock().await;
        s.messages
            .pop_front()
            .ok_or_else(|| OperatorError::Protocol("no messages queued".into()))
    }

    async fn init_upload(
        &self,
        _question_id: Option<&booth_hal::QuestionId>,
    ) -> Result<UploadSlot, OperatorError> {
        let mut s = self.inner.lock().await;
        let slot = UploadSlot {
            slot_id: format!("slot-{}", s.uploads.len() + 1),
            put_url: "https://mock.invalid/upload".to_string(),
            headers: vec![],
        };
        s.uploads.push(slot.clone());
        Ok(slot)
    }

    async fn put_upload(&self, _slot: &UploadSlot, _local_path: &str) -> Result<(), OperatorError> {
        // Pretend the upload succeeded after a short async yield.
        tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(())
    }

    async fn complete_upload(
        &self,
        _slot_id: &str,
        _sha256_hex: &str,
        _duration_ms: u64,
    ) -> Result<(), OperatorError> {
        Ok(())
    }

    async fn put_status(&self, status: BoothStatus) -> Result<(), OperatorError> {
        self.inner.lock().await.statuses.push(status);
        Ok(())
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
