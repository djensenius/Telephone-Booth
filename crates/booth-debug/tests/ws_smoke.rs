//! WebSocket smoke tests for booth-debug.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;
use std::io;
use std::time::Duration;

use booth_debug::DebugConfig;
use booth_hal::TelemetryEvent;
use futures_util::StreamExt;
use serde_json::Value;
use tokio_tungstenite::connect_async;

#[tokio::test]
async fn websocket_streams_live_telemetry() -> Result<(), Box<dyn Error>> {
    let server = common::spawn(DebugConfig::default()).await?;
    let ws_url = format!(
        "{}/v1/ws/telemetry",
        server.base_url.replacen("http://", "ws://", 1)
    );
    let (mut socket, _response) = connect_async(ws_url).await?;

    server.bus.publish(TelemetryEvent::Log {
        level: "info".to_string(),
        target: "ws_smoke".to_string(),
        message: "hello websocket".to_string(),
    });

    let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await?
        .ok_or_else(|| io::Error::other("websocket closed before telemetry frame"))??;
    let text = frame.into_text()?;
    let value: Value = serde_json::from_str(&text)?;

    assert_eq!(value.get("id").and_then(Value::as_u64), Some(1));
    assert_eq!(value.get("kind").and_then(Value::as_str), Some("log"));
    assert_eq!(
        value.get("message").and_then(Value::as_str),
        Some("hello websocket")
    );

    Ok(())
}
