//! Runtime loop integration tests with mock adapters.

use std::error::Error;

use booth_bin::{RuntimeOptions, build_mock_adapters, spawn_runtime};
use booth_core::{Event, State};
use booth_debug::RuntimeCommand;
use booth_hal::TelemetryEvent;
use booth_telemetry::TelemetryBus;
use tokio::sync::oneshot;

#[tokio::test]
async fn runtime_accepts_debug_events_and_dispatches_effects() -> Result<(), Box<dyn Error>> {
    let mut config = booth_bin::RuntimeConfig::default();
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
    assert_eq!(state, State::PlayingInstructions);

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
    let mut config = booth_bin::RuntimeConfig::default();
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
    let mut config = booth_bin::RuntimeConfig::default();
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

    // Drive to DialTone → dial 1 → FetchRandomQuestion (fast this time).
    inject(&runtime.commands, Event::HookOff).await?;
    inject(&runtime.commands, Event::RotaryPulse).await?;
    inject(&runtime.commands, Event::Tick).await?;

    // Wait for QuestionReady to be produced by the fast fetch.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let state = snapshot(&runtime.commands).await?;
        if matches!(state, State::PlayingQuestion { .. }) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("never reached PlayingQuestion state".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // PlaybackEnded → Beep, PlaybackEnded → Recording.
    inject(&runtime.commands, Event::PlaybackEnded).await?;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    inject(&runtime.commands, Event::PlaybackEnded).await?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let state = snapshot(&runtime.commands).await?;
        if matches!(state, State::Recording { .. }) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("never reached Recording state".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

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

    // Drive to Recording: dial 1 → question → beep → record.
    inject(&runtime.commands, Event::HookOff).await?;
    inject(&runtime.commands, Event::RotaryPulse).await?;
    inject(&runtime.commands, Event::Tick).await?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if matches!(
            snapshot(&runtime.commands).await?,
            State::PlayingQuestion { .. }
        ) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("never reached PlayingQuestion".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    inject(&runtime.commands, Event::PlaybackEnded).await?;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    inject(&runtime.commands, Event::PlaybackEnded).await?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if matches!(snapshot(&runtime.commands).await?, State::Recording { .. }) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("never reached Recording".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

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
