//! Verifies that the debug server releases bound ports after graceful shutdown.

#[path = "common/mod.rs"]
mod common;

use std::error::Error;
use std::time::Duration;

use booth_debug::DebugConfig;
use tokio::net::TcpListener;

/// After signalling shutdown, the debug server should release its port
/// so a new listener can rebind it immediately.
#[tokio::test]
async fn shutdown_releases_port() -> Result<(), Box<dyn Error>> {
    let server = common::spawn(DebugConfig::default()).await?;

    let addr = server
        .base_url
        .strip_prefix("http://")
        .ok_or("missing http:// prefix")?
        .to_string();

    // The port should be in use while the server is running.
    let probe = TcpListener::bind(&addr).await;
    assert!(probe.is_err(), "port should be in use before shutdown");

    // Signal graceful shutdown and wait for it to complete.
    let result = tokio::time::timeout(Duration::from_secs(5), server.shutdown()).await;
    assert!(result.is_ok(), "shutdown should complete within timeout");

    // The port should now be free for rebinding.
    let rebound = TcpListener::bind(&addr).await;
    assert!(
        rebound.is_ok(),
        "port should be released after shutdown, but got: {:?}",
        rebound.err()
    );

    Ok(())
}
