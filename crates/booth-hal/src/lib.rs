//! Hardware Abstraction Layer for the Telephone Booth phone client.
//!
//! This crate defines the **ports** in the project's hexagonal architecture.
//! The pure [`booth_core`](../booth_core/index.html) state machine emits
//! [`Effect`](crate::Effect) values that a runtime translates into calls on
//! the traits defined here. Concrete **adapters** — one for the Raspberry Pi
//! using `rppal` + `cpal` + `reqwest`, mock adapters for host testing, and any
//! future ESP32 / RP2040 adapter — live in their own crates and implement
//! these traits.
//!
//! The trait set is intentionally small and serializable so it can be exposed
//! over the debug surface for inspection, recorded in telemetry, and replayed
//! in tests.
//!
//! # Feature flags
//!
//! - `std` (default): enables `std::error::Error` impls and `#[async_trait]`-
//!   based async traits. Disable for `no_std + alloc` targets.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

/// Identifier for an audio file known to the operator (UUID-like, opaque).
pub type AudioId = String;

/// Identifier the operator assigns to a question (whose recorded answer we
/// will associate with it on upload).
pub type QuestionId = String;

/// Identifier for a recording captured locally and held for upload.
pub type RecordingId = String;

/// A monotonically increasing event id used by the telemetry ring buffer.
pub type EventSeq = u64;

// ---------------------------------------------------------------------------
// GPIO
// ---------------------------------------------------------------------------

/// Logical role a GPIO pin plays in the booth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinRole {
    /// Pulses each time a rotary digit pulse completes (one pulse per unit).
    RotaryPulse,
    /// Goes high while the rotary dial is being read (gates `RotaryPulse`).
    RotaryRead,
    /// Tracks the hook switch — `true` = on hook (idle), `false` = off hook.
    Hook,
}

/// A logical (not BCM) edge transition observed on a configured pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpioEdge {
    /// Which role this pin plays.
    pub role: PinRole,
    /// New logical level after debounce.
    pub level: bool,
    /// Nanoseconds since the runtime started.
    pub at_monotonic_ns: u64,
}

/// Errors a [`GpioPort`] implementation can return.
#[derive(Debug, thiserror::Error)]
pub enum GpioError {
    /// The pin could not be configured (already in use, permission denied,
    /// invalid BCM number, ...).
    #[error("gpio configuration failed: {0}")]
    Setup(Cow<'static, str>),
    /// The pin stream was lost or the underlying device closed.
    #[error("gpio stream lost: {0}")]
    Stream(Cow<'static, str>),
}

/// Object-safe handle that yields debounced edge events for configured pins.
///
/// The runtime treats the stream as the authoritative source of GPIO events;
/// the underlying adapter is responsible for debouncing.
///
/// Implementations should never `unwrap` and should propagate transient
/// hardware errors through [`GpioError::Stream`] without dropping the stream.
#[cfg(feature = "std")]
#[async_trait::async_trait]
pub trait GpioPort: Send + Sync {
    /// Wait for the next debounced edge.
    async fn next_edge(&mut self) -> Result<GpioEdge, GpioError>;

    /// Current sampled level of a configured pin, for diagnostic snapshots.
    async fn snapshot(&self, role: PinRole) -> Result<bool, GpioError>;
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// A reference to an audio source the [`AudioSink`] can play.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioRef {
    /// Built-in tone embedded in the binary (e.g. dial tone or beep).
    Builtin(BuiltinTone),
    /// Locally cached file (absolute path or platform-relative).
    LocalFile(String),
    /// HTTP(S) URL fetched from the operator backend or its blob store.
    RemoteUrl(String),
}

/// Built-in audio tones embedded in the binary so the booth can produce them
/// without any operator-side dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinTone {
    /// Continuous North-American 350 + 440 Hz dial tone.
    DialTone,
    /// Short "go ahead" beep before recording starts.
    Beep,
    /// Slow busy / line-busy signal.
    LineBusy,
}

/// Errors an [`AudioSink`] / [`AudioSource`] can return.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// No suitable audio device was found.
    #[error("no audio device available: {0}")]
    NoDevice(Cow<'static, str>),
    /// The device disappeared or the driver returned an error.
    #[error("audio device error: {0}")]
    Device(Cow<'static, str>),
    /// The supplied [`AudioRef`] could not be located / decoded.
    #[error("audio source unavailable: {0}")]
    Source(Cow<'static, str>),
    /// Encode or decode failure.
    #[error("audio codec error: {0}")]
    Codec(Cow<'static, str>),
    /// I/O while writing a recording.
    #[error("recording I/O error: {0}")]
    Io(Cow<'static, str>),
}

/// Telemetry sample emitted by the audio adapter at a fixed cadence (≈50 ms)
/// so debug surfaces can render a level meter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AudioLevel {
    /// Whether this sample is from the input or output device.
    pub channel: AudioChannel,
    /// Peak sample magnitude in `[0.0, 1.0]`.
    pub peak: f32,
    /// RMS sample magnitude in `[0.0, 1.0]`.
    pub rms: f32,
    /// Nanoseconds since the runtime started.
    pub at_monotonic_ns: u64,
}

/// Which side of the audio path a [`AudioLevel`] reading came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioChannel {
    /// The recording / capture device (microphone in the handset).
    Input,
    /// The playback device (handset earpiece).
    Output,
}

/// Plays audio to the booth's output device (the earpiece speaker).
#[cfg(feature = "std")]
#[async_trait::async_trait]
pub trait AudioSink: Send + Sync {
    /// Start playing `source`. If something else is playing it is replaced.
    async fn play(&mut self, source: AudioRef) -> Result<(), AudioError>;

    /// Stop any in-flight playback (no-op if nothing is playing).
    async fn stop(&mut self) -> Result<(), AudioError>;

    /// Wait until the currently-playing source has finished naturally. Returns
    /// immediately if nothing is playing. Cancel-safe: if a new `play` is
    /// invoked the future may be dropped and the new one used.
    async fn wait_for_end(&mut self) -> Result<(), AudioError>;
}

/// Captures audio from the booth's input device (the handset mouthpiece) to a
/// FLAC file on disk and yields a [`RecordingId`] when stopped.
#[cfg(feature = "std")]
#[async_trait::async_trait]
pub trait AudioSource: Send + Sync {
    /// Begin a new recording. Returns the assigned recording id.
    async fn start(&mut self) -> Result<RecordingId, AudioError>;

    /// Stop the in-flight recording (if any) and flush it to disk.
    async fn stop(&mut self) -> Result<Option<RecordingId>, AudioError>;

    /// Path of a finished recording, by id.
    async fn path_of(&self, id: &RecordingId) -> Result<String, AudioError>;
}

// ---------------------------------------------------------------------------
// Operator client
// ---------------------------------------------------------------------------

/// One element of the random-question response from the operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorQuestion {
    /// Stable id of the question.
    pub id: QuestionId,
    /// Direct, time-limited URL for the question's audio (FLAC or MP3).
    pub audio_url: String,
    /// Human-readable description (for debug logging).
    pub description: Option<String>,
}

/// One element of the random-message response from the operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorMessage {
    /// Stable id of the message.
    pub id: String,
    /// Direct, time-limited URL for the message audio.
    pub audio_url: String,
    /// Question this message answers (if any).
    pub question_id: Option<QuestionId>,
}

/// Slot the operator allocates for a forthcoming upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSlot {
    /// Opaque slot id; pass back to `complete_upload`.
    pub slot_id: String,
    /// Presigned URL (Azure SAS) the client PUTs the recording to.
    pub put_url: String,
    /// Suggested HTTP headers to include with the PUT.
    pub headers: Vec<(String, String)>,
}

/// Coarse status broadcast from the phone client to the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoothStatus {
    /// On hook, dial tone silent.
    Idle,
    /// Off hook, dial tone playing.
    DialTone,
    /// Playing a question prompt.
    PlayingQuestion,
    /// Recording an answer.
    Recording,
    /// Uploading a recording.
    Uploading,
    /// Playing a previously approved message.
    PlayingMessage,
    /// Playing the instructions prompt.
    PlayingInstructions,
}

/// Errors talking to the operator backend.
#[derive(Debug, thiserror::Error)]
pub enum OperatorError {
    /// Network / transport failure.
    #[error("operator transport error: {0}")]
    Transport(Cow<'static, str>),
    /// Authentication failed (bad / expired token).
    #[error("operator auth error: {0}")]
    Auth(Cow<'static, str>),
    /// The operator returned a non-success response we cannot recover from.
    #[error("operator returned an error: {status} {body}")]
    Server {
        /// HTTP status code returned.
        status: u16,
        /// Truncated response body for diagnostics.
        body: String,
    },
    /// We were given a malformed or unexpected response.
    #[error("operator returned an unexpected response: {0}")]
    Protocol(Cow<'static, str>),
}

/// REST + WebSocket port for the operator backend.
#[cfg(feature = "std")]
#[async_trait::async_trait]
pub trait OperatorClient: Send + Sync {
    /// Fetch a random question and bump its play-count counter on the
    /// operator side.
    async fn random_question(&self) -> Result<OperatorQuestion, OperatorError>;

    /// Fetch a random previously-approved message.
    async fn random_message(&self) -> Result<OperatorMessage, OperatorError>;

    /// Reserve an upload slot for a recording answering `question_id`.
    async fn init_upload(
        &self,
        question_id: Option<&QuestionId>,
    ) -> Result<UploadSlot, OperatorError>;

    /// PUT the bytes of `local_path` to `slot.put_url`.
    async fn put_upload(&self, slot: &UploadSlot, local_path: &str) -> Result<(), OperatorError>;

    /// Notify the operator that the upload at `slot_id` is complete and
    /// metadata (duration, sha256) is attached.
    async fn complete_upload(
        &self,
        slot_id: &str,
        sha256_hex: &str,
        duration_ms: u64,
    ) -> Result<(), OperatorError>;

    /// Push a coarse status snapshot.
    async fn put_status(&self, status: BoothStatus) -> Result<(), OperatorError>;
}

// ---------------------------------------------------------------------------
// Clock + Storage
// ---------------------------------------------------------------------------

/// Monotonic and wall-clock time for the runtime.
#[cfg(feature = "std")]
pub trait Clock: Send + Sync {
    /// Nanoseconds since the runtime started.
    fn monotonic_ns(&self) -> u64;
    /// Wall-clock unix epoch milliseconds.
    fn unix_ms(&self) -> u64;
}

/// Minimal key-value store for persisting tiny configuration bits the booth
/// needs across reboots (e.g. last-seen operator URL, debug cert fingerprint).
#[cfg(feature = "std")]
#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    /// Read a value by key.
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    /// Write or replace a value.
    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError>;
    /// Delete a value.
    async fn delete(&self, key: &str) -> Result<(), StorageError>;
}

/// Errors a [`Storage`] implementation can return.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Underlying I/O error.
    #[error("storage I/O error: {0}")]
    Io(Cow<'static, str>),
    /// Value did not deserialize.
    #[error("storage deserialization error: {0}")]
    Decode(Cow<'static, str>),
}

// ---------------------------------------------------------------------------
// Telemetry bus
// ---------------------------------------------------------------------------

/// One structured event published onto the telemetry bus.
///
/// HAL adapters, the core runtime, and the audio pipeline all publish
/// `TelemetryEvent`s. The debug surface subscribes to drive the live UI and
/// the WebSocket stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TelemetryEvent {
    /// A raw GPIO edge observed by the HAL adapter (post-debounce).
    GpioEdge(GpioEdge),
    /// A fully-decoded rotary digit (0..=9).
    DigitDialed {
        /// Digit value, 0..=9.
        digit: u8,
        /// Number of pulses that decoded into this digit.
        pulses: u8,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// The state machine moved from `from` to `to` because of `cause`.
    StateTransition {
        /// State machine state name before the event.
        from: String,
        /// State machine state name after the event.
        to: String,
        /// Cause (event kind that triggered it).
        cause: String,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// Periodic level meter sample for the input or output device.
    AudioLevel(AudioLevel),
    /// The audio device was (re)selected / changed underfoot.
    AudioDeviceChange {
        /// Human-readable device name (best effort).
        name: String,
        /// Whether the change was to the input or output side.
        channel: AudioChannel,
    },
    /// Outbound request to the operator (id, route, method).
    OperatorRequest {
        /// Correlation id (short opaque string).
        id: String,
        /// `GET /v1/...` style label.
        route: String,
    },
    /// Inbound response from the operator.
    OperatorResponse {
        /// Correlation id of the matching request.
        id: String,
        /// HTTP status code returned.
        status: u16,
        /// Duration of the round trip, milliseconds.
        duration_ms: u64,
    },
    /// A free-form structured log line, surfaced for the debug UI.
    Log {
        /// Tracing level as a lowercase string (`error`, `warn`, `info`, ...).
        level: String,
        /// Tracing target (module path).
        target: String,
        /// Rendered message.
        message: String,
    },
    /// An error that did not propagate (recoverable / dropped).
    Error {
        /// Where the error came from.
        source: String,
        /// Display-formatted error.
        message: String,
    },
}

impl fmt::Display for BoothStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => f.write_str("idle"),
            Self::DialTone => f.write_str("dial_tone"),
            Self::PlayingQuestion => f.write_str("playing_question"),
            Self::Recording => f.write_str("recording"),
            Self::Uploading => f.write_str("uploading"),
            Self::PlayingMessage => f.write_str("playing_message"),
            Self::PlayingInstructions => f.write_str("playing_instructions"),
        }
    }
}
