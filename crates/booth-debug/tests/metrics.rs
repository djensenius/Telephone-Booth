//! Tests for the loopback `/metrics` route.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;

use booth_debug::{DebugConfig, DebugToken};

const FAKE_METRICS_BODY: &str = "# HELP booth_test_total Test counter.\n\
                                 # TYPE booth_test_total counter\n\
                                 booth_test_total{booth_id=\"test-booth\"} 7\n";

#[tokio::test]
async fn metrics_route_is_served_on_loopback_without_auth() -> Result<(), Box<dyn Error>> {
    // Even when a bearer token is configured, the loopback `/metrics`
    // route must remain accessible: vmagent scrapes it without
    // credentials. The Tailscale ACL gates loopback access at the
    // network layer.
    let config = DebugConfig {
        token: Some(DebugToken("super-secret-token".to_string())),
        loopback_skip_auth: false,
        ..DebugConfig::default()
    };
    let render = common::static_metrics(FAKE_METRICS_BODY);
    let server = common::spawn_with_metrics(config, Some(render)).await?;

    let response = reqwest::Client::new()
        .get(format!("{}/metrics", server.base_url))
        .send()
        .await?
        .error_for_status()?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/plain"),
        "unexpected content-type for /metrics: {content_type}"
    );
    let body = response.text().await?;
    assert!(
        body.contains("booth_test_total"),
        "expected fake metrics body, got {body}"
    );

    Ok(())
}

#[tokio::test]
async fn metrics_route_missing_when_render_not_provided() -> Result<(), Box<dyn Error>> {
    // When booth-bin disables observability (no metrics handle), the
    // route must not exist — we don't want 200 OK with an empty body
    // confusing scrapers.
    let server = common::spawn(DebugConfig::default()).await?;
    let status = reqwest::Client::new()
        .get(format!("{}/metrics", server.base_url))
        .send()
        .await?
        .status();
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn metrics_route_authed_routes_still_require_token() -> Result<(), Box<dyn Error>> {
    // Sanity check: merging the unguarded /metrics router into the
    // loopback service must not bleed auth-bypass onto the authed
    // routes.
    let config = DebugConfig {
        token: Some(DebugToken("required-token".to_string())),
        loopback_skip_auth: false,
        ..DebugConfig::default()
    };
    let render = common::static_metrics(FAKE_METRICS_BODY);
    let server = common::spawn_with_metrics(config, Some(render)).await?;

    let status = reqwest::Client::new()
        .get(format!("{}/v1/state", server.base_url))
        .send()
        .await?
        .status();
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    Ok(())
}
