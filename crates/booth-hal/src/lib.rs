//! Hardware Abstraction Layer for the Telephone Booth phone client.
//!
//! This crate defines the **ports** in the project's hexagonal architecture.
//! The pure [`booth_core`](../booth_core/index.html) state machine emits
//! `Effect` values that a runtime translates into calls on
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
    /// GPIO is not available on this platform or build configuration.
    #[error("gpio unsupported: {0}")]
    Unsupported(Cow<'static, str>),
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
    ///
    /// The optional second field carries the expected SHA-256 hex digest of the
    /// audio bytes. When present, the adapter must verify the downloaded content
    /// before playback.
    RemoteUrl(String, Option<String>),
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
    /// The requested audio operation is unavailable for this build or adapter.
    #[error("audio operation unsupported: {0}")]
    Unsupported(Cow<'static, str>),
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

    /// Duration in milliseconds of a finished recording, if known.
    ///
    /// Adapters that cannot determine duration without re-decoding (e.g. when
    /// recovering from a spool file) may return `None`.
    async fn duration_of(&self, _id: &RecordingId) -> Option<u64> {
        None
    }

    /// Remove cached metadata for a recording that has been fully uploaded.
    ///
    /// Implementations that use durable storage with its own eviction policy
    /// may leave the default no-op in place.
    async fn cleanup_recording(&self, _id: &RecordingId) -> Result<(), AudioError> {
        Ok(())
    }
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
    /// SHA-256 digest of the question audio, when the operator supplied it.
    pub audio_sha256: Option<String>,
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
    /// SHA-256 digest of the message audio, when the operator supplied it.
    pub audio_sha256: Option<String>,
    /// Question this message answers (if any).
    pub question_id: Option<QuestionId>,
}

/// Metadata the client sends when reserving an upload slot.
///
/// Carries the recording's content-hash, size, and (when available) duration.
///
/// The current `/v1/messages` create call sends only the hash and duration;
/// size stays local for phone-side caps because the operator reads blob length
/// when the upload is completed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadMetadata {
    /// Lowercase hex SHA-256 of the recording bytes.
    pub sha256_hex: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Recording duration in milliseconds, if known.
    pub duration_ms: Option<u64>,
}

/// Slot the operator allocates for a forthcoming message upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSlot {
    /// Opaque message id; pass back to `complete_upload`.
    pub id: String,
    /// Presigned URL (Azure SAS) the client PUTs the recording to.
    pub upload_url: String,
    /// Operator-side blob name reserved for the recording.
    pub blob_name: String,
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
    /// Authentication failed (bad / expired token); rotate the configured token.
    #[error("operator unauthorized: {0}")]
    Unauthorized(Cow<'static, str>),
    /// The operator already has this recording; safe to treat as success.
    #[error("duplicate recording: {0}")]
    DuplicateRecording(Cow<'static, str>),
    /// The request is invalid and should not be retried without changing inputs.
    #[error("operator invalid argument: {0}")]
    InvalidArgument(Cow<'static, str>),
    /// The operator rejected the request because it conflicts with current state.
    #[error("operator conflict: {0}")]
    Conflict(Cow<'static, str>),
    /// The uploaded audio exceeds the operator's accepted size cap.
    #[error("operator payload too large: {body}")]
    PayloadTooLarge {
        /// Maximum byte count accepted by the operator, when reported.
        max_bytes: Option<u64>,
        /// Truncated response body for diagnostics.
        body: String,
    },
    /// The operator rejected the completed upload during validation.
    #[error("operator validation error: {0}")]
    Unprocessable(Cow<'static, str>),
    /// This adapter was compiled without support for the requested operation.
    #[error("operator operation unsupported: {0}")]
    Unsupported(Cow<'static, str>),
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
    ///
    /// `metadata` carries the recording's SHA-256, byte size, and duration so
    /// the operator can populate blob metadata and run content-addressed
    /// deduplication.
    async fn init_upload(
        &self,
        question_id: Option<&QuestionId>,
        metadata: &UploadMetadata,
    ) -> Result<UploadSlot, OperatorError>;

    /// PUT the bytes of `local_path` to `slot.upload_url`.
    async fn put_upload(&self, slot: &UploadSlot, local_path: &str) -> Result<(), OperatorError>;

    /// Notify the operator that the upload at `slot_id` is complete.
    async fn complete_upload(
        &self,
        slot_id: &str,
        sha256_hex: &str,
        duration_ms: u64,
    ) -> Result<(), OperatorError>;

    /// Push a coarse status snapshot.
    async fn put_status(&self, status: BoothStatus) -> Result<(), OperatorError>;

    /// Push a batch of telemetry events to the operator for durable
    /// persistence and live fan-out. The body is already serialized JSON
    /// shaped as `{ "events": [BoothEventWire, …] }` so the trait doesn't
    /// have to be generic over the wire encoding. Idempotency is the
    /// operator's responsibility (it deduplicates on
    /// `(boothId, eventId)`); the caller may safely retry on transport
    /// errors. Default implementation returns
    /// [`OperatorError::Unsupported`] so adapters that have no operator
    /// connection (e.g. embedded test stubs) can opt out cheaply.
    async fn push_events_json(&self, _body: &str) -> Result<EventBatchAck, OperatorError> {
        Err(OperatorError::Unsupported(
            "push_events_json not supported by this client".into(),
        ))
    }

    /// Push the latest live system snapshot to the operator. The operator
    /// keeps only the most recent snapshot per booth in-memory; this is
    /// **not** persisted. Default implementation returns
    /// [`OperatorError::Unsupported`].
    async fn put_system_snapshot(
        &self,
        _booth_id: &str,
        _snapshot: &SystemSnapshot,
    ) -> Result<(), OperatorError> {
        Err(OperatorError::Unsupported(
            "put_system_snapshot not supported by this client".into(),
        ))
    }
}

/// Acknowledgement returned by `POST /v1/events` after a bulk insert.
///
/// `accepted` is the number of newly persisted events; `duplicates` is the
/// number that the operator already had on file (same `(boothId, eventId)`)
/// and silently dropped. Callers should add the two together when
/// computing "this batch is durable", and only retry the batch if the call
/// errored out before any response was received.
#[cfg(feature = "std")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBatchAck {
    /// Newly inserted events.
    pub accepted: u32,
    /// Events that were already present and silently dropped.
    #[serde(default)]
    pub duplicates: u32,
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
// Runtime mode
// ---------------------------------------------------------------------------

/// How the booth process is wired up at runtime.
///
/// Distinct from the booth's call state ([`BoothStatus`]): this describes
/// **how** the binary is running rather than what it is doing right now.
/// Surfaced to the operator so the UI can flag non-production booths
/// (e.g. with a `MOCK` / `SIM` badge) and so Grafana can filter dashboards
/// to exclude synthetic traffic.
///
/// Precedence at startup (see `booth-bin`): if both `--simulator` and
/// `--mock` are active, the effective mode is [`RuntimeMode::Simulator`]
/// — the TUI taking over input is the more user-visible fact than the
/// mock adapters running underneath. The simulator can be paired with
/// either mock or real backend adapters; the mode reflects the input
/// surface, not the I/O backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    /// Production wiring: real GPIO / audio / HTTP adapters.
    #[default]
    Real,
    /// `booth-mock` adapters wired throughout; no real hardware or
    /// network I/O. Typical on a developer laptop or CI.
    Mock,
    /// The TUI simulator is driving input. Backend adapters may still be
    /// real (audio + operator HTTP) or mock — the mode reflects only
    /// that the human-input surface is synthetic.
    Simulator,
}

impl RuntimeMode {
    /// Stable wire-format string (matches the serde representation).
    ///
    /// Useful for metric labels and log fields that want a `&'static str`
    /// without going through `serde_json`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Real => "real",
            Self::Mock => "mock",
            Self::Simulator => "simulator",
        }
    }
}

impl fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// System snapshot
// ---------------------------------------------------------------------------

/// Live host-vitals snapshot collected periodically by the booth.
///
/// Every field is optional so adapters that cannot read a given metric (for
/// example macOS reading the Pi's `thermal_zone0`) can leave it `None`
/// without breaking the wire format. The snapshot is produced by
/// `booth-metrics` and consumed by the debug surface (`/v1/system`), the
/// operator API (`PUT /v1/system`), and the operator UI's Live System
/// panel.
///
/// New fields can be added over time without breaking older clients
/// because everything is optional and serde uses field names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSnapshot {
    /// CPU utilization and load averages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuStats>,
    /// CPU temperature in degrees Celsius (Pi `thermal_zone0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_celsius: Option<f32>,
    /// Memory in use vs total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryStats>,
    /// Per-mountpoint disk usage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<DiskStats>,
    /// Per-interface network counters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkStats>,
    /// Host uptime in seconds since boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    /// Stats for the booth process itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessStats>,
    /// Currently-selected audio devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioDeviceStats>,
    /// Tailscale link summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale: Option<TailscaleStats>,
    /// Pi throttling / undervoltage flags (`vcgencmd get_throttled`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throttling: Option<ThrottlingFlags>,
    /// How the booth process is wired up at runtime. `None` when the
    /// snapshot is taken in a context that does not know the mode (older
    /// booths predating this field, or unit tests that build snapshots
    /// directly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<RuntimeMode>,
}

/// CPU utilization plus load averages.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuStats {
    /// Overall usage ratio in `[0.0, 1.0]` (averaged across cores).
    pub usage_ratio: f32,
    /// Per-core usage ratios in `[0.0, 1.0]`, ordered by core index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_core_usage_ratio: Vec<f32>,
    /// Number of physical cores reported by the OS.
    pub physical_cores: u16,
    /// 1-minute load average.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_avg_1m: Option<f32>,
    /// 5-minute load average.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_avg_5m: Option<f32>,
    /// 15-minute load average.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_avg_15m: Option<f32>,
}

/// Memory usage in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    /// Total physical memory.
    pub total_bytes: u64,
    /// Memory currently in use (non-cache).
    pub used_bytes: u64,
    /// Total swap space.
    pub swap_total_bytes: u64,
    /// Swap currently in use.
    pub swap_used_bytes: u64,
}

/// One mounted filesystem's usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskStats {
    /// Mountpoint path, e.g. `/`.
    pub mount_point: String,
    /// Filesystem type (e.g. `ext4`, `apfs`).
    pub filesystem: String,
    /// Total filesystem size in bytes.
    pub total_bytes: u64,
    /// Bytes currently free.
    pub available_bytes: u64,
}

/// One network interface's cumulative byte counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStats {
    /// Interface name (`eth0`, `wlan0`, `en0`, ...).
    pub interface: String,
    /// Cumulative received bytes since boot.
    pub receive_bytes_total: u64,
    /// Cumulative transmitted bytes since boot.
    pub transmit_bytes_total: u64,
}

/// Stats for the booth process itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStats {
    /// Resident-set-size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resident_bytes: Option<u64>,
    /// Virtual memory in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_bytes: Option<u64>,
    /// Open file descriptor count, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_fds: Option<u32>,
    /// Thread count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    /// Process uptime in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
}

/// Currently-selected audio devices, when known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceStats {
    /// Input device name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_device: Option<String>,
    /// Output device name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// Configured sample rate, Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
}

/// Summary of the booth's Tailscale link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleStats {
    /// True when the daemon reports `BackendState: Running`.
    pub connected: bool,
    /// Number of peers, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_count: Option<u32>,
    /// Tailnet hostname this booth advertises, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Currently-used exit node hostname, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_node: Option<String>,
}

/// Raspberry Pi throttling / undervoltage flags from `vcgencmd get_throttled`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThrottlingFlags {
    /// Currently undervoltage.
    pub undervoltage: bool,
    /// Currently in arm-frequency-capped state.
    pub arm_freq_capped: bool,
    /// Currently being thermal-throttled.
    pub throttled: bool,
    /// Soft temperature limit currently active.
    pub soft_temp_limit: bool,
    /// Undervoltage occurred since boot.
    pub undervoltage_occurred: bool,
    /// Throttling occurred since boot.
    pub throttled_occurred: bool,
}

// ---------------------------------------------------------------------------
// Call outcome
// ---------------------------------------------------------------------------

/// Terminal outcome of one pickup-to-hangup call session.
///
/// Determined by `booth-bin`'s session tracker at hangup time. The set is
/// closed: every call ends in exactly one of these states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    /// Caller hung up while in dial tone / dialing, before any digit was
    /// fully decoded.
    HungUpBeforeDial,
    /// Caller hung up while a prompt (question, message, instructions) was
    /// playing.
    HungUpDuringPrompt,
    /// Caller hung up while recording their answer.
    HungUpDuringRecording,
    /// Caller hung up while the recording was uploading.
    HungUpDuringUpload,
    /// Recording was made and uploaded successfully.
    RecordingCompleted,
    /// Recording failed on the booth side (audio I/O, codec, ...).
    RecordingFailed,
    /// Upload to the operator failed terminally.
    UploadFailed,
    /// Pre-prompt operator interaction (e.g. fetching a random question)
    /// failed and the call was aborted.
    OperatorError,
    /// Any other terminal path not covered above.
    Aborted,
}

impl fmt::Display for CallOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HungUpBeforeDial => f.write_str("hung_up_before_dial"),
            Self::HungUpDuringPrompt => f.write_str("hung_up_during_prompt"),
            Self::HungUpDuringRecording => f.write_str("hung_up_during_recording"),
            Self::HungUpDuringUpload => f.write_str("hung_up_during_upload"),
            Self::RecordingCompleted => f.write_str("recording_completed"),
            Self::RecordingFailed => f.write_str("recording_failed"),
            Self::UploadFailed => f.write_str("upload_failed"),
            Self::OperatorError => f.write_str("operator_error"),
            Self::Aborted => f.write_str("aborted"),
        }
    }
}

// ---------------------------------------------------------------------------
// Telemetry bus
// ---------------------------------------------------------------------------

/// One structured event published onto the telemetry bus.
///
/// HAL adapters, the core runtime, and the audio pipeline all publish
/// `TelemetryEvent`s. The debug surface subscribes to drive the live UI and
/// the WebSocket stream. Use the `booth-telemetry` crate as the canonical
/// in-process bus and replay-ring implementation for these payloads.
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
    /// Live host-vitals snapshot, emitted periodically by `booth-metrics`.
    SystemSample {
        /// Captured system snapshot. Boxed to keep the enum small.
        snapshot: Box<SystemSnapshot>,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// A call session started: receiver went off hook.
    CallStarted {
        /// UUIDv4 minted by the runtime for this pickup-to-hangup cycle.
        session_id: String,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// A call session ended: receiver went back on hook (or otherwise
    /// terminated). Exactly one `CallEnded` is emitted per `CallStarted`.
    CallEnded {
        /// Matching session id from the preceding `CallStarted`.
        session_id: String,
        /// Terminal outcome of the call.
        outcome: CallOutcome,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// Recording of the caller's answer began.
    RecordingStarted {
        /// Adapter-assigned id for this recording.
        id: RecordingId,
        /// Session this recording belongs to.
        session_id: String,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// Recording of the caller's answer finished.
    RecordingStopped {
        /// Adapter-assigned id for this recording.
        id: RecordingId,
        /// Session this recording belongs to.
        session_id: String,
        /// Recording length, milliseconds.
        duration_ms: u64,
        /// Recording file size, bytes.
        bytes: u64,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// Upload to the operator started.
    UploadStarted {
        /// Recording being uploaded.
        recording_id: RecordingId,
        /// Session this upload belongs to.
        session_id: String,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// Upload to the operator completed successfully.
    UploadCompleted {
        /// Recording that was uploaded.
        recording_id: RecordingId,
        /// Session this upload belongs to.
        session_id: String,
        /// Time spent uploading, milliseconds.
        duration_ms: u64,
        /// Bytes uploaded.
        bytes: u64,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
    },
    /// Upload to the operator failed terminally.
    UploadFailed {
        /// Recording that was being uploaded.
        recording_id: RecordingId,
        /// Session this upload belongs to.
        session_id: String,
        /// Display-formatted error.
        message: String,
        /// Nanoseconds since runtime start.
        at_monotonic_ns: u64,
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

// ---------------------------------------------------------------------------
// URL redaction
// ---------------------------------------------------------------------------

/// Redact sensitive parts of a URL (query string, fragment, userinfo) so it is
/// safe to include in error messages, telemetry, and debug logs.
///
/// Preserves `scheme://host/path` for diagnostics. When a query string or
/// fragment is present the returned string ends with `?<redacted>`. Userinfo
/// (`user:pass@`) is stripped entirely.
///
/// If the input does not look like a URL (no `://`), it is returned unchanged.
pub fn redact_url(url: &str) -> Cow<'_, str> {
    // Fast path: no scheme separator means it's not a URL we need to redact.
    let Some(scheme_end) = url.find("://") else {
        return Cow::Borrowed(url);
    };

    let authority_start = scheme_end + 3;
    let after_scheme = &url[authority_start..];

    // Strip userinfo (everything before the first unbracketed `@` that
    // precedes the path separator).
    let (authority_and_rest, userinfo_present) = {
        // Find path start (`/`) to scope the `@` search to authority only.
        let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
        let authority_portion = &after_scheme[..path_start];
        authority_portion
            .rfind('@')
            .map_or((after_scheme, false), |at_pos| {
                (&after_scheme[at_pos + 1..], true)
            })
    };

    // Find the first `?` or `#` — everything from there onward is sensitive.
    let has_query_or_fragment =
        authority_and_rest.contains('?') || authority_and_rest.contains('#');

    if !has_query_or_fragment && !userinfo_present {
        return Cow::Borrowed(url);
    }

    // Rebuild: scheme + "://" + (authority+path without query/fragment)
    let scheme = &url[..scheme_end];
    let clean_end = authority_and_rest
        .find('?')
        .or_else(|| authority_and_rest.find('#'))
        .unwrap_or(authority_and_rest.len());
    let clean_part = &authority_and_rest[..clean_end];

    let mut redacted = String::with_capacity(scheme.len() + 3 + clean_part.len() + 11);
    redacted.push_str(scheme);
    redacted.push_str("://");
    redacted.push_str(clean_part);
    if has_query_or_fragment {
        redacted.push_str("?<redacted>");
    }
    Cow::Owned(redacted)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test assertions use expect for clearer panic messages on failure"
)]
mod tests {
    use super::*;

    #[test]
    fn strips_query_string() {
        let url = "https://storage.example.com/audio/clip.flac?sig=secret&se=2024-01-01";
        assert_eq!(
            redact_url(url),
            "https://storage.example.com/audio/clip.flac?<redacted>"
        );
    }

    #[test]
    fn strips_fragment() {
        let url = "https://cdn.example.com/path#token=abc123";
        assert_eq!(redact_url(url), "https://cdn.example.com/path?<redacted>");
    }

    #[test]
    fn strips_userinfo() {
        let url = "https://user:password@host.example.com/resource";
        assert_eq!(redact_url(url), "https://host.example.com/resource");
    }

    #[test]
    fn strips_userinfo_and_query() {
        let url = "https://user:pass@host.example.com/path?key=val";
        assert_eq!(redact_url(url), "https://host.example.com/path?<redacted>");
    }

    #[test]
    fn preserves_clean_url() {
        let url = "https://api.example.com/v1/questions/random";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn preserves_non_url_string() {
        let input = "not a url at all";
        assert_eq!(redact_url(input), input);
    }

    #[test]
    fn handles_empty_string() {
        assert_eq!(redact_url(""), "");
    }

    #[test]
    fn preserves_path_only_url_no_query() {
        let url = "http://localhost:8080/v1/audio/tone.flac";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn runtime_mode_serializes_snake_case() {
        let json = serde_json::to_string(&RuntimeMode::Real).expect("serialize real");
        assert_eq!(json, "\"real\"");
        let json = serde_json::to_string(&RuntimeMode::Mock).expect("serialize mock");
        assert_eq!(json, "\"mock\"");
        let json = serde_json::to_string(&RuntimeMode::Simulator).expect("serialize simulator");
        assert_eq!(json, "\"simulator\"");
    }

    #[test]
    fn runtime_mode_round_trips() {
        for mode in [RuntimeMode::Real, RuntimeMode::Mock, RuntimeMode::Simulator] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let parsed: RuntimeMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(mode, parsed);
            assert_eq!(mode.as_str(), parsed.as_str());
        }
    }

    #[test]
    fn runtime_mode_default_is_real() {
        assert_eq!(RuntimeMode::default(), RuntimeMode::Real);
    }

    #[test]
    fn system_snapshot_omits_runtime_mode_when_none() {
        let snapshot = SystemSnapshot::default();
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(
            !json.contains("runtimeMode"),
            "default snapshot should omit runtimeMode, got: {json}"
        );
    }

    #[test]
    fn system_snapshot_emits_runtime_mode_when_set() {
        let snapshot = SystemSnapshot {
            runtime_mode: Some(RuntimeMode::Simulator),
            ..SystemSnapshot::default()
        };
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(
            json.contains("\"runtimeMode\":\"simulator\""),
            "snapshot should carry simulator mode, got: {json}"
        );
    }
}
