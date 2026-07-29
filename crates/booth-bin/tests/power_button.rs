//! Power-button integration tests over the mock runtime.
//!
//! These exercise the runtime's `power_button_task` timing end-to-end: a short
//! press must synthesize `Event::PowerButtonPressed` -> `Effect::Reboot`, and a
//! hold past the configured threshold must synthesize `Event::PowerButtonHeld`
//! -> `Effect::PowerOff`. The mock power controller records the requested
//! action instead of touching the host, so nothing actually reboots.

use std::error::Error;
use std::time::Duration;

use booth_bin::{RuntimeOptions, build_mock_adapters, spawn_runtime};
use booth_debug::RuntimeCommand;
use booth_hal::TelemetryEvent;
use booth_mock::PowerAction;
use booth_telemetry::TelemetryBus;

/// Wait until the runtime has published its first status-LED telemetry event,
/// which happens once the runtime loop is live and processing effects.
async fn wait_for_runtime_ready(bus: &TelemetryBus) -> Result<(), Box<dyn Error>> {
    for _ in 0..200 {
        if bus
            .snapshot_since(None)
            .iter()
            .any(|r| matches!(r.event, TelemetryEvent::StatusLed { .. }))
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("timed out waiting for the runtime to become ready".into())
}

/// Poll `actions()` until it is non-empty or the deadline elapses.
async fn wait_for_action(
    power: &booth_mock::MockPowerController,
) -> Result<Vec<PowerAction>, Box<dyn Error>> {
    for _ in 0..300 {
        let actions = power.actions().await;
        if !actions.is_empty() {
            return Ok(actions);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("timed out waiting for a power action".into())
}

#[tokio::test]
async fn short_press_reboots() -> Result<(), Box<dyn Error>> {
    let mut config = booth_bin::RuntimeConfig::default();
    config.power_button.enabled = true;
    config.power_button.hold_ms = 1_000;
    let bus = TelemetryBus::new(128);
    let (adapters, handles) = build_mock_adapters(&bus);
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

    wait_for_runtime_ready(&bus).await?;

    // Press then release well within the hold threshold: a short press.
    handles.gpio.push_power_button(true).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    handles.gpio.push_power_button(false).await;

    let actions = wait_for_action(&handles.power).await?;
    assert_eq!(actions, vec![PowerAction::Reboot]);

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}

#[tokio::test]
async fn hold_powers_off() -> Result<(), Box<dyn Error>> {
    let mut config = booth_bin::RuntimeConfig::default();
    config.power_button.enabled = true;
    config.power_button.hold_ms = 150;
    let bus = TelemetryBus::new(128);
    let (adapters, handles) = build_mock_adapters(&bus);
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

    wait_for_runtime_ready(&bus).await?;

    // Press and hold: the task fires PowerButtonHeld once the threshold elapses,
    // without waiting for a release. (Not pushing a release keeps the assertion
    // independent of edge-delivery timing.)
    handles.gpio.push_power_button(true).await;

    let actions = wait_for_action(&handles.power).await?;
    assert_eq!(actions, vec![PowerAction::PowerOff]);

    handles.gpio.push_power_button(false).await;
    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}
