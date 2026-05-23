//! HTTP smoke tests for booth-debug.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;

use booth_debug::DebugConfig;
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
