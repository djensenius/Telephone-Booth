#![allow(dead_code, missing_docs)]

use std::error::Error;
use std::io;
use std::sync::Arc;

use booth_debug::{
    DebugConfig, MetricsRender, RuntimeCommand, ServeHandles, TelemetryBus, serve_with_handles,
};
use tokio::sync::mpsc;

pub struct TestServer {
    pub base_url: String,
    pub bus: TelemetryBus,
    pub rx: mpsc::Receiver<RuntimeCommand>,
    handles: Option<ServeHandles>,
}

impl TestServer {
    /// Signal graceful shutdown and consume the server, returning
    /// once all listener tasks have completed.
    pub async fn shutdown(mut self) {
        if let Some(handles) = self.handles.take() {
            let _ = handles.shutdown_tx.send(());
            let _ = handles.handle.await;
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handles) = self.handles.as_ref() {
            handles.handle.abort();
        }
    }
}

pub async fn spawn(config: DebugConfig) -> Result<TestServer, Box<dyn Error>> {
    spawn_with_metrics(config, None).await
}

pub async fn spawn_with_metrics(
    mut config: DebugConfig,
    metrics_render: Option<MetricsRender>,
) -> Result<TestServer, Box<dyn Error>> {
    config.loopback_bind = "127.0.0.1:0".to_string();
    config.lan_enabled = false;
    config.tailscale_enabled = true;
    config.ring_buffer_capacity = 32;

    let bus = TelemetryBus::new(config.ring_buffer_capacity);
    let (tx, rx) = mpsc::channel(32);
    let handles = serve_with_handles(config, bus.clone(), tx, metrics_render).await?;
    let addr = handles
        .loopback_addr
        .ok_or_else(|| io::Error::other("loopback listener did not start"))?;
    Ok(TestServer {
        base_url: format!("http://{addr}"),
        bus,
        rx,
        handles: Some(handles),
    })
}

/// Build a `MetricsRender` that returns a fixed Prometheus text body. Used
/// by tests to assert routing without bringing in `booth-metrics`.
pub fn static_metrics(body: &'static str) -> MetricsRender {
    Arc::new(move || body.to_string())
}
