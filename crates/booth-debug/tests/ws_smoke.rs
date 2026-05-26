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
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

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

#[tokio::test]
async fn websocket_echoes_bearer_subprotocol() -> Result<(), Box<dyn Error>> {
    // Browsers (notably Safari) drop the WebSocket upgrade with "The network
    // connection was lost" if the client offered a Sec-WebSocket-Protocol and
    // the server didn't echo one back. The simulator UI smuggles the bearer
    // token through that header as `bearer.<token>`, so the server must
    // acknowledge it for the handshake to complete in real browsers.
    let server = common::spawn(DebugConfig::default()).await?;
    let ws_url = format!(
        "{}/v1/ws/telemetry",
        server.base_url.replacen("http://", "ws://", 1)
    );
    let mut request = ws_url.as_str().into_client_request()?;
    let header_value = "bearer.test-token".parse()?;
    request
        .headers_mut()
        .insert("sec-websocket-protocol", header_value);

    let (_socket, response) = connect_async(request).await?;
    let echoed = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok());
    assert_eq!(echoed, Some("bearer.test-token"));

    Ok(())
}
