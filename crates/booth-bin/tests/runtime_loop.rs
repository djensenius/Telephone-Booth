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
