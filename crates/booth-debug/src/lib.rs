//! Embedded debug surface for the Telephone Booth phone client.
//!
//! Exposes a small HTTP / WebSocket API for inspecting the state machine, GPIO
//! pins, audio levels, recent logs, and live raw telemetry. Designed to sit
//! behind `tailscale serve` (which terminates TLS with a real Let's Encrypt
//! cert) on the primary path, with a self-signed-cert LAN fallback for direct
//! browser access. See the operator repo docs for the wire schema.
//!
//! The actual routes, telemetry bus implementation, and embedded htmx UI are
//! filled in by the parallel agent task `rust-debug-surface`; this module
//! currently exports the type shape so other crates can compile against it.

#![warn(missing_docs)]

pub use booth_telemetry::{TelemetryBus, TelemetryRecord};

use serde::{Deserialize, Serialize};

/// Bearer debug token loaded from `/etc/phone-booth/debug-token`.
#[derive(Debug, Clone)]
pub struct DebugToken(pub String);

/// Configuration for the debug surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugConfig {
    /// Bind address for the loopback listener proxied by `tailscale serve`.
    /// Defaults to `127.0.0.1:8080`.
    #[serde(default = "default_loopback")]
    pub loopback_bind: String,
    /// Bind address for the LAN-fallback TLS listener. Defaults to
    /// `0.0.0.0:8443`.
    #[serde(default = "default_lan")]
    pub lan_bind: String,
    /// Whether to expose the loopback endpoint for `tailscale serve`.
    #[serde(default = "default_true")]
    pub tailscale_enabled: bool,
    /// Whether to expose the LAN-fallback HTTPS endpoint.
    #[serde(default = "default_true")]
    pub lan_enabled: bool,
    /// Whether `POST /debug/simulate/*` and similar control endpoints are
    /// available. Defaults to `false` so production deployments cannot be
    /// driven remotely without an explicit operator decision.
    #[serde(default)]
    pub allow_controls: bool,
    /// Maximum number of telemetry events retained for catch-up. Defaults to
    /// 4096.
    #[serde(default = "default_ring")]
    pub ring_buffer_capacity: usize,
}

fn default_loopback() -> String {
    "127.0.0.1:8080".into()
}
fn default_lan() -> String {
    "0.0.0.0:8443".into()
}
fn default_true() -> bool {
    true
}
fn default_ring() -> usize {
    4096
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            loopback_bind: default_loopback(),
            lan_bind: default_lan(),
            tailscale_enabled: true,
            lan_enabled: true,
            allow_controls: false,
            ring_buffer_capacity: default_ring(),
        }
    }
}

/// Errors the debug surface can return at startup.
#[derive(Debug, thiserror::Error)]
pub enum DebugError {
    /// Could not bind the requested socket.
    #[error("bind failed: {0}")]
    Bind(String),
    /// TLS certificate or key generation/load failed.
    #[error("tls error: {0}")]
    Tls(String),
}

/// Snapshot returned by `GET /debug/state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// Current state tag (`idle`, `dial_tone`, ...).
    pub state: String,
    /// Last N state transitions, newest-first.
    pub recent_transitions: Vec<TelemetryRecord>,
}

// NOTE: route handlers, the axum router, the in-memory telemetry bus, and the
// rust-embed-backed standalone htmx UI live in submodules added by the
// `rust-debug-surface` and `rust-debug-telemetry-bus` agent tasks. This stub
// keeps the workspace compiling end-to-end.
