//! Log capture tests for booth-debug.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;

use booth_debug::{DebugConfig, log_layer};
use serde_json::Value;
use tracing_subscriber::prelude::*;

#[tokio::test]
async fn tracing_layer_feeds_logs_endpoint() -> Result<(), Box<dyn Error>> {
    let _subscriber = tracing_subscriber::registry()
        .with(log_layer())
        .set_default();
    let server = common::spawn(DebugConfig::default()).await?;

    tracing::info!(target: "booth_debug_test", answer = 42, "log endpoint smoke");

    let logs: Value = reqwest::get(format!("{}/v1/logs?level=info&limit=5", server.base_url))
        .await?
        .error_for_status()?
        .json()
        .await?;
    let entries = logs
        .as_array()
        .ok_or_else(|| std::io::Error::other("logs response was not an array"))?;
    assert!(entries.iter().any(|entry| {
        entry
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("log endpoint smoke"))
    }));

    Ok(())
}
