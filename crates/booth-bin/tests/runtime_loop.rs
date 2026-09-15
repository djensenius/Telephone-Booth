//! Runtime loop integration tests with mock adapters.

use std::error::Error;

use booth_bin::{RuntimeOptions, build_mock_adapters, spawn_runtime};
use booth_core::{Event, State};
use booth_debug::RuntimeCommand;
use booth_hal::{AudioRef, BuiltinTone, TelemetryEvent};
use booth_telemetry::TelemetryBus;
use tokio::sync::oneshot;

#[tokio::test(start_paused = true)]
async fn exhibition_end_defers_inflight_recording_and_restart_replays_without_reboot()
-> Result<(), Box<dyn Error>> {
    use booth_bin::pending_uploads::PendingUploadSpool;
    use booth_hal::InstallationState;
    use std::time::Duration;

    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    config.observability.enabled = false;
    let bus = TelemetryBus::new(512);
    let (adapters, handles) = build_mock_adapters(&bus);
    handles.operator.enable_installation_state();
    {
        let state = handles.operator.state();
        let mut state = state.lock().await;
        state.installation_state = Some(InstallationState::Active);
        state.questions.push_back(booth_hal::OperatorQuestion {
            id: "question".into(),
            audio_url: "https://mock.invalid/question.flac".into(),
            audio_sha256: None,
            description: None,
        });
    }
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
    wait_for_state(&runtime.commands, "active idle", |s| *s == State::Idle).await?;
    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;
    handles.operator.state().lock().await.installation_state =
        Some(InstallationState::BetweenExhibitions);
    tokio::time::advance(Duration::from_secs(6)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        snapshot(&runtime.commands).await?,
        State::Recording { .. }
    ));
    inject(&runtime.commands, Event::HookOn).await?;
    wait_for_state(&runtime.commands, "paused after finalizing", |s| {
        matches!(s, State::CallsPaused { on_hook: true })
    })
    .await?;
    let spool = PendingUploadSpool::open(dir.path().join("pending-uploads"))?;
    assert_eq!(spool.scan().len(), 1);
    assert!(handles.operator.state().lock().await.uploads.is_empty());
    let playback_count = handles.audio_sink.state().await.history.len();
    inject(&runtime.commands, Event::HookOff).await?;
    for digit in 0..=9 {
        inject(&runtime.commands, Event::RotaryPulse).await?;
        inject(&runtime.commands, Event::DigitDialed { digit }).await?;
    }
    assert_eq!(
        handles.audio_sink.state().await.history.len(),
        playback_count
    );
    assert!(!bus.snapshot_since(None).iter().any(|record| matches!(
        record.event,
        TelemetryEvent::UploadCompleted { .. } | TelemetryEvent::UploadFailed { .. }
    )));
    handles.operator.state().lock().await.installation_state = Some(InstallationState::Active);
    tokio::time::advance(Duration::from_secs(6)).await;
    wait_for_state(&runtime.commands, "resumed dial tone", |s| {
        *s == State::DialTone
    })
    .await?;
    for _ in 0..200 {
        if spool.scan().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(spool.scan().is_empty());
    assert_eq!(handles.operator.state().lock().await.uploads.len(), 1);
    assert_eq!(snapshot(&runtime.commands).await?, State::DialTone);
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    runtime.join.await??;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn upload_pickup_survives_inactive_deferral_and_restart() -> Result<(), Box<dyn Error>> {
    use booth_bin::observability::SessionTracker;
    use booth_bin::pending_uploads::PendingUploadSpool;
    use booth_hal::{InstallationState, OperatorError};
    use std::time::Duration;

    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    config.observability.enabled = false;
    let bus = TelemetryBus::new(512);
    let (adapters, handles) = build_mock_adapters(&bus);
    handles.operator.enable_installation_state();
    {
        let state = handles.operator.state();
        let mut state = state.lock().await;
        state.installation_state = Some(InstallationState::Active);
        state.questions.push_back(booth_hal::OperatorQuestion {
            id: "question".into(),
            audio_url: "https://mock.invalid/question.flac".into(),
            audio_sha256: None,
            description: None,
        });
    }
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
    wait_for_state(&runtime.commands, "active idle", |s| *s == State::Idle).await?;
    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;
    {
        let state = handles.operator.state();
        let mut state = state.lock().await;
        state.latency = Some(Duration::from_millis(500));
        state.fail_complete_upload = Some(OperatorError::InstallationInactive(
            "ended during upload".into(),
        ));
    }
    inject(&runtime.commands, Event::HookOn).await?;
    wait_for_state(&runtime.commands, "on-hook upload", |s| {
        matches!(s, State::Uploading { on_hook: true, .. })
    })
    .await?;
    let spool = PendingUploadSpool::open(dir.path().join("pending-uploads"))?;
    for _ in 0..100 {
        if !spool.scan().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pending = spool.scan();
    assert_eq!(pending.len(), 1);
    let playback_count = handles.audio_sink.state().await.history.len();

    inject(&runtime.commands, Event::HookOff).await?;
    assert_eq!(
        snapshot(&runtime.commands).await?,
        State::Uploading {
            recording_id: pending[0].recording_id.clone(),
            question_id: "question".into(),
            on_hook: false,
        }
    );
    handles.operator.state().lock().await.installation_state =
        Some(InstallationState::BetweenExhibitions);
    wait_for_state(&runtime.commands, "off-hook deferred upload", |s| {
        matches!(s, State::CallsPaused { on_hook: false })
    })
    .await?;
    inject(&runtime.commands, Event::RotaryPulse).await?;
    inject(&runtime.commands, Event::Tick).await?;
    assert_eq!(
        snapshot(&runtime.commands).await?,
        State::CallsPaused { on_hook: false }
    );
    assert_eq!(
        PendingUploadSpool::open(dir.path().join("pending-uploads"))?.scan(),
        pending
    );
    assert_eq!(handles.operator.state().lock().await.uploads.len(), 1);
    assert_eq!(
        handles.audio_sink.state().await.history.len(),
        playback_count
    );
    let records = bus.snapshot_since(None);
    assert!(!records.iter().any(|record| matches!(
        record.event,
        TelemetryEvent::UploadCompleted { .. }
            | TelemetryEvent::UploadFailed { .. }
            | TelemetryEvent::Error { .. }
    )));
    let mut tracker = SessionTracker::new();
    let call_events: Vec<_> = records
        .iter()
        .flat_map(|record| tracker.observe(&record.event, 0))
        .collect();
    assert!(matches!(
        call_events.as_slice(),
        [
            TelemetryEvent::CallStarted { .. },
            TelemetryEvent::CallEnded { .. }
        ]
    ));
    let last_record = records.last().ok_or("missing runtime telemetry")?.id;

    {
        let state = handles.operator.state();
        let mut state = state.lock().await;
        state.installation_state = Some(InstallationState::Active);
        state.fail_complete_upload = None;
        state.latency = None;
    }
    tokio::time::advance(Duration::from_secs(6)).await;
    wait_for_state(&runtime.commands, "resumed off-hook dial tone", |s| {
        *s == State::DialTone
    })
    .await?;
    wait_for_playback(&handles.audio_sink, "resumed dial tone", |source| {
        matches!(source, AudioRef::Builtin(BuiltinTone::DialTone))
    })
    .await?;
    for _ in 0..100 {
        if spool.scan().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(spool.scan().is_empty());
    assert_eq!(handles.operator.state().lock().await.uploads.len(), 2);
    assert_eq!(snapshot(&runtime.commands).await?, State::DialTone);
    assert_eq!(
        handles.audio_sink.state().await.history.len(),
        playback_count + 1
    );
    let resumed_call_events: Vec<_> = bus
        .snapshot_since(Some(last_record))
        .iter()
        .flat_map(|record| tracker.observe(&record.event, 0))
        .collect();
    assert!(matches!(
        resumed_call_events.as_slice(),
        [TelemetryEvent::CallStarted { .. }]
    ));
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    runtime.join.await??;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn completion_conflict_preserves_startup_spool_and_replays_on_manual_start()
-> Result<(), Box<dyn Error>> {
    use booth_bin::pending_uploads::{PendingUploadSpool, SpoolEntry};
    use booth_hal::{InstallationState, OperatorError};
    use std::time::Duration;
    let dir = tempfile::tempdir()?;
    let recording = dir.path().join("answer.flac");
    std::fs::write(&recording, b"retained-recording")?;
    let spool = PendingUploadSpool::open(dir.path().join("pending-uploads"))?;
    spool.enqueue(&SpoolEntry {
        recording_id: "answer".into(),
        question_id: Some("question".into()),
        path: recording.to_string_lossy().into_owned(),
        size_bytes: Some(18),
        duration_ms: Some(5000),
    })?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.observability.enabled = false;
    let bus = TelemetryBus::new(512);
    let (adapters, handles) = build_mock_adapters(&bus);
    handles.operator.enable_installation_state();
    {
        let state = handles.operator.state();
        let mut state = state.lock().await;
        state.installation_state = Some(InstallationState::BetweenExhibitions);
    }
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
    wait_for_state(&runtime.commands, "startup paused", |s| {
        matches!(s, State::CallsPaused { .. })
    })
    .await?;
    tokio::time::advance(Duration::from_secs(31)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(handles.operator.state().lock().await.uploads.is_empty());
    assert_eq!(spool.scan().len(), 1);
    {
        let state = handles.operator.state();
        let mut state = state.lock().await;
        state.installation_state = Some(InstallationState::Active);
        state.fail_complete_upload = Some(OperatorError::InstallationInactive(
            "ended during upload".into(),
        ));
    }
    tokio::time::advance(Duration::from_secs(6)).await;
    for _ in 0..100 {
        if !handles.operator.state().lock().await.uploads.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    handles.operator.state().lock().await.installation_state =
        Some(InstallationState::BetweenExhibitions);
    wait_for_state(&runtime.commands, "completion deferred", |s| {
        matches!(s, State::CallsPaused { .. })
    })
    .await?;
    assert_eq!(spool.scan().len(), 1);
    assert_eq!(std::fs::read(&recording)?, b"retained-recording");
    assert!(!bus.snapshot_since(None).iter().any(|record| matches!(
        record.event,
        TelemetryEvent::UploadCompleted { .. }
            | TelemetryEvent::UploadFailed { .. }
            | TelemetryEvent::Error { .. }
    )));
    {
        let state = handles.operator.state();
        let mut state = state.lock().await;
        state.installation_state = Some(InstallationState::Active);
        state.fail_complete_upload = None;
    }
    tokio::time::advance(Duration::from_secs(6)).await;
    for _ in 0..100 {
        if spool.scan().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(spool.scan().is_empty());
    assert_eq!(handles.operator.state().lock().await.uploads.len(), 2);
    assert_eq!(snapshot(&runtime.commands).await?, State::Idle);
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    runtime.join.await??;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn slow_startup_status_never_blocks_gpio_or_power_controls() -> Result<(), Box<dyn Error>> {
    use std::time::Duration;
    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    config.observability.enabled = false;
    let bus = TelemetryBus::new(128);
    let (adapters, handles) = build_mock_adapters(&bus);
    handles.operator.enable_installation_state();
    handles.operator.state().lock().await.installation_latency = Some(Duration::from_mins(1));
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
    inject(&runtime.commands, Event::HookOff).await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(200), snapshot(&runtime.commands)).await??,
        State::CallsPaused { on_hook: false }
    );
    inject(&runtime.commands, Event::PowerButtonPressed).await?;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        handles.power.actions().await,
        vec![booth_mock::PowerAction::Reboot]
    );
    tokio::time::advance(Duration::from_secs(6)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let state = handles.operator.state();
    assert!(state.lock().await.installation_checks <= 2);
    assert!(
        bus.snapshot_since(None)
            .iter()
            .any(|record| matches!(&record.event,
        TelemetryEvent::Error { source, .. } if source == "installation_state"))
    );
    assert!(handles.audio_sink.state().await.history.is_empty());
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    runtime.join.await??;
    Ok(())
}

#[tokio::test]
async fn immediate_shutdown_persists_queued_recording_without_waiting_for_network()
-> Result<(), Box<dyn Error>> {
    use booth_bin::pending_uploads::PendingUploadSpool;
    use booth_hal::AudioSource;
    use std::time::Duration;
    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    config.observability.enabled = false;
    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);
    handles
        .operator
        .state()
        .lock()
        .await
        .questions
        .push_back(booth_hal::OperatorQuestion {
            id: "question".into(),
            audio_url: "https://mock.invalid/question.flac".into(),
            audio_sha256: None,
            description: None,
        });
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
    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;
    wait_for_log(&bus, "recording started").await?;
    let recording_id = handles
        .audio_source
        .clone()
        .stop()
        .await?
        .ok_or("missing recording")?;
    handles.operator.state().lock().await.latency = Some(Duration::from_mins(1));
    runtime
        .commands
        .send(RuntimeCommand::InjectEvent(Event::RecordingFinished {
            recording_id: recording_id.clone(),
        }))
        .await?;
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    tokio::time::timeout(Duration::from_secs(5), runtime.join).await???;
    let pending = PendingUploadSpool::open(dir.path().join("pending-uploads"))?.scan();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].recording_id, recording_id);
    assert_eq!(pending[0].question_id.as_deref(), Some("question"));
    assert!(handles.operator.state().lock().await.uploads.is_empty());
    Ok(())
}

#[tokio::test]
async fn runtime_accepts_debug_events_and_dispatches_effects() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    let bus = TelemetryBus::new(128);
    let (adapters, _handles) = build_mock_adapters(&bus);
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

    inject(&runtime.commands, Event::HookOff).await?;
    for _ in 0..3 {
        inject(&runtime.commands, Event::RotaryPulse).await?;
    }
    inject(&runtime.commands, Event::Tick).await?;
    let state = snapshot(&runtime.commands).await?;
    assert_eq!(state, State::CallUnavailable);

    inject(&runtime.commands, Event::HookOn).await?;
    inject(&runtime.commands, Event::HookOff).await?;
    for _ in 0..2 {
        inject(&runtime.commands, Event::RotaryPulse).await?;
    }
    inject(&runtime.commands, Event::Tick).await?;

    wait_for_message_request(&bus).await?;
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _final_state = runtime.join.await??;
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

async fn snapshot(
    commands: &tokio::sync::mpsc::Sender<RuntimeCommand>,
) -> Result<State, Box<dyn Error>> {
    let (tx, rx) = oneshot::channel();
    commands.send(RuntimeCommand::Snapshot(tx)).await?;
    Ok(rx.await?)
}

async fn wait_for_message_request(bus: &TelemetryBus) -> Result<(), Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let seen = bus.snapshot_since(None).into_iter().any(|record| {
            matches!(
                record.event,
                TelemetryEvent::OperatorRequest { route, .. } if route.contains("random-message")
            )
        });
        if seen {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("operator random-message request was not observed".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Verify that a hangup (`HookOn`) during a slow `FetchRandomQuestion` is not
/// blocked: `StopAudio` is processed immediately and the state transitions to
/// `Idle` within a tight deadline.
#[tokio::test]
async fn hangup_during_slow_fetch_is_not_blocked() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);

    // Inject 2 seconds of latency into the mock operator so
    // FetchRandomQuestion takes a long time.
    handles.operator.state().lock().await.latency = Some(std::time::Duration::from_secs(2));

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

    // Drive to DialTone, then dial 1 to trigger FetchRandomQuestion.
    inject(&runtime.commands, Event::HookOff).await?;
    inject(&runtime.commands, Event::RotaryPulse).await?;
    inject(&runtime.commands, Event::Tick).await?;

    // Give the effect task a moment to start processing FetchRandomQuestion.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Hang up while the slow fetch is in-flight.
    inject(&runtime.commands, Event::HookOn).await?;

    // The state machine should transition to Idle immediately (within 200ms)
    // because StopAudio/CancelPulseTimeout are on the critical path, not
    // blocked behind the 2-second operator call.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(200);
    loop {
        let state = snapshot(&runtime.commands).await?;
        if state == State::Idle {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "state did not return to Idle within 200ms — hangup blocked by slow operator"
                    .into(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

/// Verify that a hangup during a slow upload does not block critical effects.
/// The upload takes 2 seconds but `StopAudio` fires within 200ms.
#[tokio::test]
async fn hangup_during_slow_upload_is_not_blocked() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);

    // Queue a question so FetchRandomQuestion succeeds quickly at first.
    {
        let state = handles.operator.state();
        let mut s = state.lock().await;
        s.questions.push_back(booth_hal::OperatorQuestion {
            id: "q-1".to_string(),
            audio_url: "https://mock.invalid/q1.flac".to_string(),
            audio_sha256: None,
            description: None,
        });
    }

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

    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;

    // RecordingFinished → Uploading (triggers UploadRecording effect).
    // Before injecting RecordingFinished, add latency so the upload is slow.
    handles.operator.state().lock().await.latency = Some(std::time::Duration::from_secs(2));

    inject(
        &runtime.commands,
        Event::RecordingFinished {
            recording_id: "rec-000001".to_string(),
        },
    )
    .await?;

    // Give effect_task time to pick up UploadRecording and start the slow upload.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Hang up while the upload is in progress.
    inject(&runtime.commands, Event::HookOn).await?;

    // State should reach Idle within 200ms (not after the 2s upload).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(200);
    loop {
        let state = snapshot(&runtime.commands).await?;
        if state == State::Idle {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "state did not return to Idle within 200ms — hangup blocked by slow upload".into(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

/// A recording shorter than `audio.min_recording_secs` must be discarded, not
/// uploaded: no upload slot is issued and the booth returns to a dial tone.
#[tokio::test]
async fn short_recording_is_discarded_without_upload() -> Result<(), Box<dyn Error>> {
    let mut config = booth_bin::RuntimeConfig::default();
    config.debug.allow_controls = true;
    // Mock recordings report 5s; require 10s so this one is "too short".
    config.audio.min_recording_secs = 10;
    // Isolate the upload spool so a sibling test's queued upload can't be
    // recovered onto this test's operator at startup.
    let rec_dir = std::env::temp_dir().join(format!(
        "booth-discard-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&rec_dir)?;
    config.audio.recordings_dir = rec_dir.to_string_lossy().into_owned();
    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);

    {
        let state = handles.operator.state();
        let mut s = state.lock().await;
        s.questions.push_back(booth_hal::OperatorQuestion {
            id: "q-1".to_string(),
            audio_url: "https://mock.invalid/q1.flac".to_string(),
            audio_sha256: None,
            description: None,
        });
    }

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

    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;

    // Finish the (too-short) recording. The gate should discard it and the
    // booth should return to a dial tone without issuing an upload slot.
    inject(
        &runtime.commands,
        Event::RecordingFinished {
            recording_id: "rec-short".to_string(),
        },
    )
    .await?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if matches!(snapshot(&runtime.commands).await?, State::DialTone) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("booth did not return to DialTone after discarding".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        handles.operator.state().lock().await.uploads.is_empty(),
        "a short recording must not issue an upload slot"
    );

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    let _ = std::fs::remove_dir_all(&rec_dir);
    Ok(())
}

/// Hanging up must always reset the booth, even when the audio adapter has no
/// recording to finalize. Without a `RecordingFailed` fallback the runtime
/// would never emit `RecordingFinished`, leaving the state machine stuck in
/// `FinishingRecording` with no dial tone for the next caller.
#[tokio::test]
async fn hangup_with_no_recording_in_flight_resets_to_idle() -> Result<(), Box<dyn Error>> {
    use booth_hal::AudioSource;

    let mut config = booth_bin::RuntimeConfig::default();
    let dir = tempfile::tempdir()?;
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);
    {
        let state = handles.operator.state();
        let mut s = state.lock().await;
        s.questions.push_back(booth_hal::OperatorQuestion {
            id: "q-1".to_string(),
            audio_url: "https://mock.invalid/q1.flac".to_string(),
            audio_sha256: None,
            description: None,
        });
    }
    let mut audio_source = handles.audio_source.clone();

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

    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;

    // Reaching `Recording` only means the transition happened; wait until the
    // `StartRecording` effect has actually reached the adapter before taking
    // the capture away, otherwise the effect could start a fresh recording
    // after we clear it and the hangup would finalize normally.
    wait_for_log(&bus, "recording started").await?;

    // Simulate the adapter losing the recording (e.g. the capture stream died),
    // so finalizing it on hangup yields nothing to upload.
    audio_source.stop().await?;

    inject(&runtime.commands, Event::HookOn).await?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if snapshot(&runtime.commands).await? == State::Idle {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("booth stayed wedged instead of resetting to Idle on hangup".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

/// A clip fetched for a call that was abandoned mid-fetch must never be played
/// to the next caller: the hangup invalidates the in-flight fetch, so its
/// `QuestionReady` is dropped instead of pulling the next call into
/// `PlayingQuestion` on its own.
#[tokio::test]
async fn abandoned_fetch_does_not_leak_into_the_next_call() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    let bus = TelemetryBus::new(256);
    let (adapters, handles) = build_mock_adapters(&bus);
    {
        let state = handles.operator.state();
        let mut s = state.lock().await;
        s.latency = Some(std::time::Duration::from_millis(300));
        s.questions.push_back(booth_hal::OperatorQuestion {
            id: "q-abandoned".to_string(),
            audio_url: "https://mock.invalid/abandoned.flac".to_string(),
            audio_sha256: None,
            description: None,
        });
    }

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

    // Dial 1, then hang up while the question fetch is still in flight.
    inject(&runtime.commands, Event::HookOff).await?;
    inject(&runtime.commands, Event::RotaryPulse).await?;
    inject(&runtime.commands, Event::Tick).await?;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    inject(&runtime.commands, Event::HookOn).await?;

    // The next caller lifts the handset before the abandoned fetch resolves.
    inject(&runtime.commands, Event::HookOff).await?;
    assert_eq!(snapshot(&runtime.commands).await?, State::DialTone);

    // Well past the fetch latency, the booth must still be sitting on a dial
    // tone rather than playing the previous caller's question.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        snapshot(&runtime.commands).await?,
        State::DialTone,
        "a clip fetched for an abandoned call leaked into the next call"
    );

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

/// Hanging up must silence playback even while a previous call's upload is
/// still in flight. The upload used to hold the `AudioSource` lock for its
/// whole (retried) lifetime, so the next call's `StartRecording` effect blocked
/// the effect dispatcher and the `StopAudio` queued behind it never reached the
/// sink — the caller hung up and the clip kept playing.
#[tokio::test]
async fn hangup_stops_playback_while_an_upload_is_still_running() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let mut config = booth_bin::RuntimeConfig::default();
    config.audio.recordings_dir = dir.path().join("recordings").to_string_lossy().into_owned();
    config.debug.allow_controls = true;
    let bus = TelemetryBus::new(512);
    let (adapters, handles) = build_mock_adapters(&bus);
    {
        let state = handles.operator.state();
        let mut s = state.lock().await;
        for id in ["q-1", "q-2"] {
            s.questions.push_back(booth_hal::OperatorQuestion {
                id: id.to_string(),
                audio_url: format!("https://mock.invalid/{id}.flac"),
                audio_sha256: None,
                description: None,
            });
        }
    }

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

    // First call: reach Recording, then start an upload that takes a long time.
    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;
    handles.operator.state().lock().await.latency = Some(std::time::Duration::from_secs(3));
    inject(
        &runtime.commands,
        Event::RecordingFinished {
            recording_id: "rec-000001".to_string(),
        },
    )
    .await?;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    inject(&runtime.commands, Event::HookOn).await?;

    // Second call, while that upload is still running: play a question, reach
    // Recording (which needs the audio source the upload used to hold), then
    // hang up.
    handles.operator.state().lock().await.latency = None;
    drive_to_recording(&runtime.commands, &handles.audio_sink).await?;
    inject(&runtime.commands, Event::HookOn).await?;

    // The sink must go quiet well before the 3s upload finishes.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        if handles.audio_sink.state().await.playing.is_none() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("playback was still running 500ms after hangup".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

/// Drive a fresh call from on-hook through dialing 1 to `Recording`.
async fn drive_to_recording(
    commands: &tokio::sync::mpsc::Sender<RuntimeCommand>,
    audio_sink: &booth_mock::MockAudioSink,
) -> Result<(), Box<dyn Error>> {
    inject(commands, Event::HookOff).await?;
    inject(commands, Event::RotaryPulse).await?;
    inject(commands, Event::Tick).await?;
    wait_for_state(commands, "RingingQuestion", |state| {
        matches!(state, State::RingingQuestion { .. })
    })
    .await?;
    let ringback_index = wait_for_playback(audio_sink, "ringback", |source| {
        matches!(source, AudioRef::Builtin(BuiltinTone::Ringback))
    })
    .await?;
    audio_sink.finish_playback();
    wait_for_state(commands, "PlayingQuestion", |state| {
        matches!(state, State::PlayingQuestion { .. })
    })
    .await?;
    wait_for_playback(audio_sink, "remote question", |source| {
        matches!(source, AudioRef::RemoteUrl(_, _))
    })
    .await?;
    audio_sink.finish_playback();
    wait_for_state(commands, "Beep", |state| {
        matches!(state, State::Beep { .. })
    })
    .await?;
    wait_for_playback(audio_sink, "recording beep", |source| {
        matches!(source, AudioRef::Builtin(BuiltinTone::Beep))
    })
    .await?;
    audio_sink.finish_playback();
    wait_for_state(commands, "Recording", |state| {
        matches!(state, State::Recording { .. })
    })
    .await?;

    let history = audio_sink.state().await.history;
    assert!(
        matches!(
            &history[ringback_index..],
            [
                AudioRef::Builtin(BuiltinTone::Ringback),
                AudioRef::RemoteUrl(_, _),
                AudioRef::Builtin(BuiltinTone::Beep)
            ]
        ),
        "expected ringback → remote question → beep, got {:?}",
        &history[ringback_index..]
    );
    Ok(())
}

async fn wait_for_playback(
    audio_sink: &booth_mock::MockAudioSink,
    label: &str,
    predicate: impl Fn(&AudioRef) -> bool,
) -> Result<usize, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let state = audio_sink.state().await;
        if state.playing.as_ref().is_some_and(&predicate) {
            return Ok(state.history.len() - 1);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("never started {label} playback").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

async fn wait_for_state(
    commands: &tokio::sync::mpsc::Sender<RuntimeCommand>,
    label: &str,
    predicate: impl Fn(&State) -> bool + Send + Sync,
) -> Result<(), Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if predicate(&snapshot(commands).await?) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("never reached {label}").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Wait until a telemetry `Log` record containing `needle` is observed.
async fn wait_for_log(bus: &TelemetryBus, needle: &str) -> Result<(), Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let seen = bus.snapshot_since(None).into_iter().any(|record| {
            matches!(
                record.event,
                TelemetryEvent::Log { ref message, .. } if message.contains(needle)
            )
        });
        if seen {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("telemetry log containing {needle:?} was not observed").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
