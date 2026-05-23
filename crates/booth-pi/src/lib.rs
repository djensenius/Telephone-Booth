//! Raspberry Pi adapter for the Telephone Booth phone client.
//!
//! Concrete implementations of every [`booth_hal`] trait, backed by:
//!
//! * `rppal` for GPIO edge detection on configurable BCM pins.
//! * `cpal` (ALSA on the Pi) for USB-Audio-Class-2 capture and playback,
//!   notably the user's Focusrite.
//! * `flacenc` / `claxon` / `symphonia` for FLAC encode + decode.
//! * `reqwest` + `tokio-tungstenite` for talking to the operator backend.
//!
//! Hardware-only dependencies are gated behind the `pi` Cargo feature so the
//! crate still type-checks on macOS / x86_64-linux when running the workspace
//! test suite. The GPIO adapter is implemented in [`gpio`]; audio and operator
//! adapters are filled in by the remaining `rust-pi-*` agent tasks.

#![warn(missing_docs)]

use booth_hal::PinRole;
use serde::{Deserialize, Serialize};

pub mod gpio;

#[cfg(feature = "pi")]
pub use gpio::PiGpioPort;

pub mod audio;

pub use audio::{
    PiAudioSink, PiAudioSource, RecordingHandle, device_name_matches, embedded_tone_bytes,
    has_flac_stream_marker,
};

/// Pi-side configuration. Loaded from `/etc/phone-booth/config.toml` (with
/// per-key environment-variable overrides) at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiConfig {
    /// GPIO pin assignments. Defaults preserve the existing wiring of the
    /// physical 2016 installation (physical pins 11 / 13 / 15 →
    /// BCM 17 / 27 / 22).
    #[serde(default)]
    pub gpio: GpioConfig,
    /// Audio device selection.
    #[serde(default)]
    pub audio: AudioConfig,
    /// Operator backend connection.
    #[serde(default)]
    pub operator: OperatorConfig,
}

/// BCM pin assignments and electrical settings for the booth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpioConfig {
    /// Rotary pulse pin (physical 13 = BCM 27 by default).
    #[serde(default = "default_rotary_pulse", alias = "rotary_pulse_bcm")]
    pub rotary_pulse: u8,
    /// Rotary "reading" / dialing gate pin (physical 15 = BCM 22 by default).
    #[serde(
        default = "default_rotary_read",
        alias = "rotary_gate",
        alias = "rotary_gate_bcm",
        alias = "rotary_read_bcm"
    )]
    pub rotary_read: u8,
    /// Hook switch pin (physical 11 = BCM 17 by default).
    #[serde(default = "default_hook", alias = "hook_bcm")]
    pub hook: u8,
    /// Internal pull resistor applied to all configured inputs.
    #[serde(default)]
    pub pull: GpioPull,
    /// Debounce window applied to all pins.
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    /// Optional per-role inversion applied after reading the physical level.
    #[serde(default)]
    pub invert: GpioInvertConfig,
}

/// Internal pull resistor direction for GPIO inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpioPull {
    /// Enable the Raspberry Pi's internal pull-up resistor.
    Up,
    /// Enable the Raspberry Pi's internal pull-down resistor.
    Down,
}

impl Default for GpioPull {
    fn default() -> Self {
        Self::Up
    }
}

/// Per-role GPIO level inversion settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpioInvertConfig {
    /// Invert rotary pulse levels.
    #[serde(default)]
    pub rotary_pulse: bool,
    /// Invert rotary read / dialing gate levels.
    #[serde(default, alias = "rotary_gate")]
    pub rotary_read: bool,
    /// Invert hook switch levels.
    #[serde(default)]
    pub hook: bool,
}

fn default_rotary_pulse() -> u8 {
    27
}
fn default_rotary_read() -> u8 {
    22
}
fn default_hook() -> u8 {
    17
}
fn default_debounce_ms() -> u64 {
    5
}

impl Default for GpioConfig {
    fn default() -> Self {
        Self {
            rotary_pulse: default_rotary_pulse(),
            rotary_read: default_rotary_read(),
            hook: default_hook(),
            pull: GpioPull::default(),
            debounce_ms: default_debounce_ms(),
            invert: GpioInvertConfig::default(),
        }
    }
}

impl GpioConfig {
    /// Resolve a logical [`PinRole`] to its configured BCM pin.
    #[must_use]
    pub fn bcm_for(&self, role: PinRole) -> u8 {
        match role {
            PinRole::RotaryPulse => self.rotary_pulse,
            PinRole::RotaryRead => self.rotary_read,
            PinRole::Hook => self.hook,
        }
    }

    /// Return whether the physical level for `role` should be inverted.
    #[must_use]
    pub fn inverted(&self, role: PinRole) -> bool {
        match role {
            PinRole::RotaryPulse => self.invert.rotary_pulse,
            PinRole::RotaryRead => self.invert.rotary_read,
            PinRole::Hook => self.invert.hook,
        }
    }
}

/// Audio configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Match audio device by name substring (e.g. `"Focusrite"`). If unset,
    /// uses the system default.
    #[serde(default = "default_device_substring", alias = "device_name_substring")]
    pub device_substring: Option<String>,
    /// Recording/playback sample rate. 48000 is recommended for USB-Audio-Class-2.
    #[serde(default = "default_sample_rate")]
    pub sample_rate_hz: u32,
    /// Channel count used for handset capture and playback.
    #[serde(default = "default_channels")]
    pub channels: u16,
    /// Maximum recording duration before auto-stop, in seconds.
    #[serde(
        default = "default_max_recording_secs",
        alias = "max_recording_seconds"
    )]
    pub max_recording_secs: u32,
    /// Where to write FLAC recordings before upload.
    #[serde(default = "default_recordings_dir")]
    pub recordings_dir: String,
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "serde default must match the Option<String> field type"
)]
fn default_device_substring() -> Option<String> {
    Some("Focusrite".to_string())
}
fn default_sample_rate() -> u32 {
    48_000
}
fn default_channels() -> u16 {
    1
}
fn default_max_recording_secs() -> u32 {
    60
}
fn default_recordings_dir() -> String {
    "/var/lib/telephone-booth/recordings".to_string()
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device_substring: default_device_substring(),
            sample_rate_hz: default_sample_rate(),
            channels: default_channels(),
            max_recording_secs: default_max_recording_secs(),
            recordings_dir: default_recordings_dir(),
        }
    }
}

/// Operator backend connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorConfig {
    /// Base URL of the operator backend (e.g. `https://operator.example.com`).
    #[serde(default = "default_operator_url")]
    pub base_url: String,
    /// Bearer API token. Use `${PHONE_BOOTH_TOKEN}` to read from env at boot.
    #[serde(default)]
    pub api_token: String,
    /// Connect timeout (seconds).
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: u64,
}

fn default_operator_url() -> String {
    "https://operator.example.com".to_string()
}
fn default_connect_timeout() -> u64 {
    10
}

impl Default for OperatorConfig {
    fn default() -> Self {
        Self {
            base_url: default_operator_url(),
            api_token: String::new(),
            connect_timeout_seconds: default_connect_timeout(),
        }
    }
}

impl Default for PiConfig {
    fn default() -> Self {
        Self {
            gpio: GpioConfig::default(),
            audio: AudioConfig::default(),
            operator: OperatorConfig::default(),
        }
    }
}

// NOTE: the concrete `cpal`-backed `AudioSink` + `AudioSource`, and
// `reqwest`-backed `OperatorClient` implementations are added by the remaining
// `rust-pi-*` agent tasks. Each lives in its own submodule (`audio`, `client`)
// gated behind the `pi` feature.
