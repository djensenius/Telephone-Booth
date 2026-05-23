//! Authentication tests for booth-debug.

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
