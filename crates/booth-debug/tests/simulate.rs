//! Simulation endpoint tests for booth-debug.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;
use std::io;
use std::time::Duration;

use booth_core::Event;
use booth_debug::{DebugConfig, RuntimeCommand, RuntimeMode};

#[tokio::test]
async fn simulate_event_is_forbidden_without_controls() -> Result<(), Box<dyn Error>> {
    let server = common::spawn(DebugConfig::default()).await?;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/simulate/event", server.base_url))
        .json(&Event::HookOff)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"], "controls_denied");
    assert_eq!(body["reason"], "controls_disabled");
    Ok(())
}

#[tokio::test]
async fn simulate_event_is_forbidden_in_real_runtime_mode() -> Result<(), Box<dyn Error>> {
    // `allow_controls` is on, but the booth is running against real hardware
    // — the second gate must still refuse the injection.
    let config = DebugConfig {
        allow_controls: true,
        runtime_mode: RuntimeMode::Real,
        ..DebugConfig::default()
    };
    let server = common::spawn(config).await?;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/simulate/event", server.base_url))
        .json(&Event::HookOff)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["error"], "controls_denied");
    assert_eq!(body["reason"], "headless_real_hardware");
    assert_eq!(body["runtimeMode"], "real");
    Ok(())
}

#[tokio::test]
async fn simulate_event_forwards_to_runtime_when_enabled() -> Result<(), Box<dyn Error>> {
    let config = DebugConfig {
        allow_controls: true,
        runtime_mode: RuntimeMode::Simulator,
        ..DebugConfig::default()
    };
    let mut server = common::spawn(config).await?;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/simulate/event", server.base_url))
        .json(&Event::HookOff)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let command = tokio::time::timeout(Duration::from_secs(2), server.rx.recv())
        .await?
        .ok_or_else(|| io::Error::other("runtime command channel closed"))?;
    assert!(matches!(
        command,
        RuntimeCommand::InjectEvent(Event::HookOff)
    ));

    Ok(())
}

#[tokio::test]
async fn config_endpoint_exposes_runtime_mode() -> Result<(), Box<dyn Error>> {
    let config = DebugConfig {
        runtime_mode: RuntimeMode::Mock,
        ..DebugConfig::default()
    };
    let server = common::spawn(config).await?;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/v1/config", server.base_url))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(body["debug"]["runtimeMode"], "mock");
    Ok(())
}
