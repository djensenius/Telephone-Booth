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
            ..RuntimeOptions::default()
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

/// Durability regression test for djensenius/Telephone-Booth#104: a booth
/// restart with events queued (and undeliverable) before shutdown must
/// deliver those events after coming back online. The first run simulates a
/// total operator outage so the buffered `CallStarted` / `CallEnded` are
/// spilled to disk on shutdown; the second run (operator reachable, same
/// recordings dir) must replay the on-disk spool and deliver them.
#[tokio::test]
async fn queued_events_survive_restart_via_spool() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();

    // --- First run: operator is unreachable for the whole run. ---
    {
        let mut config = booth_bin::RuntimeConfig::default();
        config.debug.allow_controls = true;
        config.observability.enabled = true;
        config.observability.booth_id = "booth-test".to_string();
        config.observability.operator_forward.flush_interval_ms = 100;
        config.audio.recordings_dir = recordings_dir.clone();

        let bus = TelemetryBus::new(256);
        let (adapters, handles) = build_mock_adapters(&bus);
        // Simulate a transient API/network outage for the whole first run so
        // no batch is ever acknowledged.
        handles.operator.state().lock().await.fail_events = Some(
            booth_hal::OperatorError::Transport("simulated outage".into()),
        );

        let runtime = spawn_runtime(
            config,
            adapters,
            bus.clone(),
            RuntimeOptions {
                start_debug: false,
                listen_signals: false,
                notify_systemd: false,
                ..RuntimeOptions::default()
            },
        );

        // Pickup → hangup produces CallStarted + CallEnded.
        inject(&runtime.commands, Event::HookOff).await?;
        inject(&runtime.commands, Event::HookOn).await?;

        // Give the forwarder time to attempt (and fail) at least one flush so
        // the events remain buffered in memory.
        tokio::time::sleep(Duration::from_millis(300)).await;

        runtime.commands.send(RuntimeCommand::Shutdown).await?;
        let _ = runtime.join.await?;
    }

    // The graceful shutdown should have spilled the buffered events to disk.
    let spool_dir = std::path::Path::new(&recordings_dir).join("event-spool");
    let spooled = spool_json_files(&spool_dir)?;
    assert!(
        !spooled.is_empty(),
        "expected spooled event files after shutdown during an outage; found none in {}",
        spool_dir.display()
    );

    // --- Second run: operator reachable, spool must replay on startup. ---
    let mut config = booth_bin::RuntimeConfig::default();
    config.observability.enabled = true;
    config.observability.booth_id = "booth-test".to_string();
    config.observability.operator_forward.flush_interval_ms = 100;
    config.audio.recordings_dir = recordings_dir.clone();

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
            ..RuntimeOptions::default()
        },
    );

    // Wait for the on-disk spool to be replayed to the operator.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut seen_call_ended = false;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let batches = operator.state().lock().await.event_batches.clone();
        if batches.iter().any(|b| b.contains("\"call_ended\"")) {
            seen_call_ended = true;
            break;
        }
    }
    assert!(
        seen_call_ended,
        "expected the replayed spool to deliver call_ended after restart"
    );

    // A successful replay must drain the spool from disk.
    let remaining = spool_json_files(&spool_dir)?;
    assert!(
        remaining.is_empty(),
        "expected the spool to be drained after replay; still present: {remaining:?}"
    );

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

/// Regression for the shutdown-drain race in djensenius/Telephone-Booth#106:
/// a shutdown that fires immediately after `HookOn` — with **no** delay to
/// let the forwarder pull the events out of the broadcast queue — must still
/// deliver `CallEnded`. Records queued by `handle_event` before the shutdown
/// signal are drained and persisted on the way out; without the drain the
/// synthesized `CallEnded` (which the tracker only emits once it *observes*
/// the `HookOn` transition) would never make it into the batch and would be
/// lost. Unlike `queued_events_survive_restart_via_spool`, this test omits
/// the pre-shutdown sleep so it exercises the still-queued path.
#[tokio::test]
async fn immediate_shutdown_after_hangup_spills_call_ended() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();

    let mut config = booth_bin::RuntimeConfig::default();
    config.debug.allow_controls = true;
    config.observability.enabled = true;
    config.observability.booth_id = "booth-test".to_string();
    // A long flush interval guarantees the periodic flush never fires during
    // the test window, so delivery depends solely on the shutdown drain.
    config.observability.operator_forward.flush_interval_ms = 60_000;
    config.audio.recordings_dir = recordings_dir.clone();

    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);
    // Total outage so the shutdown flush cannot succeed and the batch must be
    // spilled to disk instead — that on-disk spool is what we assert on.
    handles.operator.state().lock().await.fail_events = Some(booth_hal::OperatorError::Transport(
        "simulated outage".into(),
    ));

    let runtime = spawn_runtime(
        config,
        adapters,
        bus.clone(),
        RuntimeOptions {
            start_debug: false,
            listen_signals: false,
            notify_systemd: false,
            ..RuntimeOptions::default()
        },
    );

    // Pickup → hangup produces CallStarted + CallEnded, then shut down
    // immediately with no intervening sleep.
    inject(&runtime.commands, Event::HookOff).await?;
    inject(&runtime.commands, Event::HookOn).await?;
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;

    // The shutdown drain must have converted the queued HookOn transition into
    // a synthesized CallEnded and spilled it to disk.
    let spool_dir = std::path::Path::new(&recordings_dir).join("event-spool");
    let spooled = spool_json_files(&spool_dir)?;
    assert!(
        !spooled.is_empty(),
        "expected spooled event files after immediate shutdown; found none in {}",
        spool_dir.display()
    );
    let mut found_call_ended = false;
    for path in &spooled {
        if std::fs::read_to_string(path)?.contains("call_ended") {
            found_call_ended = true;
            break;
        }
    }
    assert!(
        found_call_ended,
        "expected the spilled spool to contain call_ended; files: {spooled:?}"
    );
    Ok(())
}

/// Collect the `*.json` spool files in `dir` (ignoring in-progress `.tmp-*`
/// files), returning an empty list if the directory does not exist yet.
fn spool_json_files(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, Box<dyn Error>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let is_json = path.extension().is_some_and(|ext| ext == "json");
        let is_hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if path.is_file() && is_json && !is_hidden {
            files.push(path);
        }
    }
    Ok(files)
}
