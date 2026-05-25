//! Authentication tests for booth-debug.

#![allow(clippy::unwrap_used)]

#[path = "common/mod.rs"]
mod common;

use std::error::Error;

use booth_debug::{DebugConfig, DebugToken};

#[tokio::test]
async fn bearer_token_is_required_when_configured() -> Result<(), Box<dyn Error>> {
    let config = DebugConfig {
        token: Some(DebugToken("secret-token".to_string())),
        ..DebugConfig::default()
    };
    let server = common::spawn(config).await?;
    let client = reqwest::Client::new();

    let missing = client
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await?;
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);

    let wrong = client
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth("wrong-token")
        .send()
        .await?;
    assert_eq!(wrong.status(), reqwest::StatusCode::UNAUTHORIZED);

    let ok = client
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth("secret-token")
        .send()
        .await?;
    assert_eq!(ok.status(), reqwest::StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn loopback_skip_auth_exempts_loopback_clients() -> Result<(), Box<dyn Error>> {
    let config = DebugConfig {
        token: Some(DebugToken("secret-token".to_string())),
        loopback_skip_auth: true,
        ..DebugConfig::default()
    };
    let server = common::spawn(config).await?;

    let response = reqwest::get(format!("{}/healthz", server.base_url)).await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn lan_rejects_external_bind_without_token() {
    let config = DebugConfig {
        tailscale_enabled: false,
        lan_enabled: true,
        lan_bind: "0.0.0.0:0".to_string(),
        token: None,
        allow_tokenless: true,
        ..DebugConfig::default()
    };

    let bus = booth_debug::TelemetryBus::new(32);
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = booth_debug::serve_with_handles(config, bus, tx, None).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("insecure lan bind"),
        "expected InsecureLanBind error, got: {err}"
    );
}

#[tokio::test]
async fn lan_rejects_external_bind_with_weak_token() {
    let config = DebugConfig {
        tailscale_enabled: false,
        lan_enabled: true,
        lan_bind: "0.0.0.0:0".to_string(),
        token: Some(DebugToken("short".to_string())),
        ..DebugConfig::default()
    };

    let bus = booth_debug::TelemetryBus::new(32);
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let result = booth_debug::serve_with_handles(config, bus, tx, None).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("too short"),
        "expected token-too-short error, got: {err}"
    );
}

#[tokio::test]
async fn lan_allows_external_bind_with_strong_token() -> Result<(), Box<dyn Error>> {
    let config = DebugConfig {
        tailscale_enabled: false,
        lan_enabled: true,
        lan_bind: "0.0.0.0:0".to_string(),
        token: Some(DebugToken("a-very-strong-token-1234".to_string())),
        ..DebugConfig::default()
    };

    let bus = booth_debug::TelemetryBus::new(32);
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let handles = booth_debug::serve_with_handles(config, bus, tx, None).await?;

    assert!(handles.lan_addr.is_some());
    let _ = handles.shutdown_tx.send(());
    let _ = handles.handle.await;
    Ok(())
}

#[tokio::test]
async fn lan_allows_loopback_bind_without_token() -> Result<(), Box<dyn Error>> {
    let config = DebugConfig {
        tailscale_enabled: false,
        lan_enabled: true,
        lan_bind: "127.0.0.1:0".to_string(),
        token: None,
        allow_tokenless: true,
        ..DebugConfig::default()
    };

    let bus = booth_debug::TelemetryBus::new(32);
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let handles = booth_debug::serve_with_handles(config, bus, tx, None).await?;

    assert!(handles.lan_addr.is_some());
    let _ = handles.shutdown_tx.send(());
    let _ = handles.handle.await;
    Ok(())
}
