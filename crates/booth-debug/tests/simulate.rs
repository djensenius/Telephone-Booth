//! Simulation endpoint tests for booth-debug.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;
use std::io;
use std::time::Duration;

use booth_core::Event;
use booth_debug::{DebugConfig, RuntimeCommand};

#[tokio::test]
async fn simulate_event_is_forbidden_without_controls() -> Result<(), Box<dyn Error>> {
    let server = common::spawn(DebugConfig::default()).await?;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/simulate/event", server.base_url))
        .json(&Event::HookOff)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn simulate_event_forwards_to_runtime_when_enabled() -> Result<(), Box<dyn Error>> {
    let config = DebugConfig {
        allow_controls: true,
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
