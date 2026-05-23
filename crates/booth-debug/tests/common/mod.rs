#![allow(dead_code, missing_docs)]

use std::error::Error;
use std::io;

use booth_debug::{DebugConfig, RuntimeCommand, ServeHandles, TelemetryBus, serve_with_handles};
use tokio::sync::mpsc;

pub struct TestServer {
    pub base_url: String,
    pub bus: TelemetryBus,
    pub rx: mpsc::Receiver<RuntimeCommand>,
    handles: ServeHandles,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handles.handle.abort();
    }
}

pub async fn spawn(mut config: DebugConfig) -> Result<TestServer, Box<dyn Error>> {
    config.loopback_bind = "127.0.0.1:0".to_string();
    config.lan_enabled = false;
    config.tailscale_enabled = true;
    config.ring_buffer_capacity = 32;

    let bus = TelemetryBus::new(config.ring_buffer_capacity);
    let (tx, rx) = mpsc::channel(32);
    let handles = serve_with_handles(config, bus.clone(), tx).await?;
    let addr = handles
        .loopback_addr
        .ok_or_else(|| io::Error::other("loopback listener did not start"))?;
    Ok(TestServer {
        base_url: format!("http://{addr}"),
        bus,
        rx,
        handles,
    })
}
