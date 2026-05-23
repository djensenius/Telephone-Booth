//! Integration tests for booth-side observability: session tracking,
//! event forwarding to the operator, and system snapshot push.

use std::error::Error;
use std::time::Duration;

use booth_bin::{RuntimeOptions, build_mock_adapters, spawn_runtime};
use booth_core::Event;
use booth_debug::RuntimeCommand;
use booth_telemetry::TelemetryBus;

#[tokio::test]
async fn observability_forwards_call_events_to_operator() -> Result<(), Box<dyn Error>> {
    let mut config = booth_bin::RuntimeConfig::default();
    config.debug.allow_controls = true;
    config.observability.enabled = true;
    config.observability.booth_id = "booth-test".to_string();
    // Push more often than the default 2s so the test stays snappy.
    config.observability.operator_forward.flush_interval_ms = 100;
    config
        .observability
        .operator_forward
        .system_push_interval_ms = 100;
    config.observability.sample_interval_ms = 100;

    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);
    let operator = handles.operator.clone();

    let runtime = spawn_runtime(
        config,
        adapters,
        bus.clone(),
        RuntimeOptions {
            start_debug: false,
            listen_signals: false,
            notify_systemd: false,
        },
    );

    // Simulate a pickup → hangup cycle.
    inject(&runtime.commands, Event::HookOff).await?;
    inject(&runtime.commands, Event::HookOn).await?;

    // Wait for the forwarder to flush at least one batch carrying the
    // CallStarted + CallEnded events.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut seen_call_started = false;
    let mut seen_call_ended = false;
    let mut last_batches: Vec<String> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let batches = operator.state().lock().await.event_batches.clone();
        last_batches = batches.clone();
        for body in &batches {
            if body.contains("\"call_started\"") {
                seen_call_started = true;
            }
            if body.contains("\"call_ended\"") {
                seen_call_ended = true;
            }
        }
        if seen_call_started && seen_call_ended {
            break;
        }
    }
    assert!(
        seen_call_started,
        "expected at least one event batch containing call_started; got batches: {last_batches:?}"
    );
    assert!(
        seen_call_ended,
        "expected at least one event batch containing call_ended; got batches: {last_batches:?}"
    );

    // The system sampler should have produced at least one PUT /v1/system.
    let mut snapshot_count = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        snapshot_count = operator.state().lock().await.system_snapshots.len();
        if snapshot_count > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        snapshot_count > 0,
        "expected at least one PUT /v1/system call"
    );

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

async fn inject(
    commands: &tokio::sync::mpsc::Sender<RuntimeCommand>,
    event: Event,
) -> Result<(), Box<dyn Error>> {
    commands.send(RuntimeCommand::InjectEvent(event)).await?;
    tokio::task::yield_now().await;
    Ok(())
}
