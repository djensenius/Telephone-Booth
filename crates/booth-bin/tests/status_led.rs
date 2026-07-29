//! Status-LED integration tests over the mock runtime.
//!
//! These cover two invariants that are easy to regress:
//!
//! 1. The transient blue "booting" pulse is replaced with the indication the
//!    core maps to the initial state once startup finishes. Without that the
//!    LED would stay on the boot pulse until the first state transition, which
//!    on an on-hook booth can be hours away.
//! 2. `TelemetryEvent::StatusLed` is published exactly once per accepted
//!    change, by the runtime rather than by the adapter, so no consumer sees
//!    duplicated indications.

use std::error::Error;
use std::time::Duration;

use booth_bin::{RuntimeOptions, build_mock_adapters, spawn_runtime};
use booth_core::{LED_BRIGHTNESS_DIM, LED_SLOW_PULSE_MS};
use booth_debug::RuntimeCommand;
use booth_hal::{LedColour, LedPattern, TelemetryEvent};
use booth_telemetry::TelemetryBus;

fn led_events(bus: &TelemetryBus) -> Vec<(LedColour, LedPattern)> {
    bus.snapshot_since(None)
        .into_iter()
        .filter_map(|record| match record.event {
            TelemetryEvent::StatusLed {
                colour, pattern, ..
            } => Some((colour, pattern)),
            _ => None,
        })
        .collect()
}

async fn wait_for_idle_indication(bus: &TelemetryBus) -> Result<(), Box<dyn Error>> {
    let idle = (
        LedColour::Green,
        LedPattern::Steady {
            brightness: LED_BRIGHTNESS_DIM,
        },
    );
    for _ in 0..300 {
        if led_events(bus).last() == Some(&idle) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("timed out waiting for the idle status-LED indication".into())
}

#[tokio::test]
async fn boot_indication_is_replaced_by_idle_without_a_transition() -> Result<(), Box<dyn Error>> {
    let bus = TelemetryBus::new(128);
    let (adapters, handles) = build_mock_adapters(&bus);
    let runtime = spawn_runtime(
        booth_bin::RuntimeConfig::default(),
        adapters,
        bus.clone(),
        RuntimeOptions {
            start_debug: false,
            listen_signals: false,
            notify_systemd: false,
            ..RuntimeOptions::default()
        },
    );

    wait_for_idle_indication(&bus).await?;

    // The adapter saw the boot pulse first, then the idle glow — with no booth
    // state transition in between.
    let history = handles.status_led.history().await;
    assert_eq!(
        history.first(),
        Some(&(
            LedColour::Blue,
            LedPattern::Pulse {
                period_ms: LED_SLOW_PULSE_MS
            }
        ))
    );
    assert_eq!(
        history.last(),
        Some(&(
            LedColour::Green,
            LedPattern::Steady {
                brightness: LED_BRIGHTNESS_DIM
            }
        ))
    );

    // Each accepted change is reported exactly once: the runtime publishes, the
    // adapter does not.
    assert_eq!(led_events(&bus), history);

    runtime.commands.send(RuntimeCommand::Shutdown).await?;
    let _ = runtime.join.await?;
    Ok(())
}
