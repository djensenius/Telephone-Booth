//! HTTP smoke tests for booth-debug.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;

use booth_debug::DebugConfig;
use booth_hal::{LedColour, LedPattern, TelemetryEvent};
use serde_json::Value;

#[tokio::test]
async fn health_state_and_events_are_served() -> Result<(), Box<dyn Error>> {
    let server = common::spawn(DebugConfig::default()).await?;
    let client = reqwest::Client::new();

    let health: Value = client
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(health.get("ok").and_then(Value::as_bool), Some(true));
    assert!(health.get("version").and_then(Value::as_str).is_some());

    let state: Value = client
        .get(format!("{}/v1/state", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(state.get("state").and_then(Value::as_str), Some("idle"));
    assert!(state.get("updatedAt").and_then(Value::as_str).is_some());

    let events: Value = client
        .get(format!("{}/v1/events?since=0", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(events.as_array().is_some_and(Vec::is_empty));

    Ok(())
}

#[tokio::test]
async fn status_led_snapshot_tracks_telemetry() -> Result<(), Box<dyn Error>> {
    let server = common::spawn(DebugConfig::default()).await?;
    let client = reqwest::Client::new();

    // Nothing observed yet: `updatedAt` is null so the UI can say "unknown"
    // instead of claiming the ring is dark.
    let empty: Value = client
        .get(format!("{}/v1/status-led", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(empty.get("updatedAt").is_some_and(Value::is_null));

    server.bus.publish(TelemetryEvent::StatusLed {
        colour: LedColour::Green,
        pattern: LedPattern::Steady { brightness: 40 },
        at_monotonic_ns: 7,
    });
    server.bus.publish(TelemetryEvent::StatusLed {
        colour: LedColour::Blue,
        pattern: LedPattern::Blink { period_ms: 300 },
        at_monotonic_ns: 9,
    });

    // The newest event wins.
    let snapshot: Value = client
        .get(format!("{}/v1/status-led", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(snapshot.get("colour").and_then(Value::as_str), Some("blue"));
    assert_eq!(
        snapshot.get("patternLabel").and_then(Value::as_str),
        Some("blink(300ms)")
    );
    assert_eq!(
        snapshot.get("atMonotonicNs").and_then(Value::as_u64),
        Some(9)
    );
    assert!(snapshot.get("updatedAt").and_then(Value::as_str).is_some());

    Ok(())
}
