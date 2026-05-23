//! Optional GPIO loopback smoke test for Raspberry Pi hardware.

#![cfg(feature = "pi")]

use std::error::Error;
use std::time::Duration;

use booth_hal::{GpioPort, PinRole};
use booth_pi::{GpioConfig, PiGpioPort};
use rppal::gpio::Gpio;

#[tokio::test]
#[ignore = "requires Raspberry Pi loopback wiring: connect BCM 23 (physical pin 16) to the hook input BCM 17 (physical pin 11), with common ground; see docs/hardware.md"]
async fn loopback_hook_edges() -> Result<(), Box<dyn Error + Send + Sync>> {
    let output_bcm = loopback_output_bcm();
    let config = GpioConfig::default();
    let gpio = Gpio::new()?;
    let mut output = gpio.get(output_bcm)?.into_output_low();
    let mut port = PiGpioPort::new(config)?;

    tokio::time::sleep(Duration::from_millis(20)).await;

    output.set_high();
    let rising = tokio::time::timeout(Duration::from_secs(1), port.next_edge()).await??;
    assert_eq!(rising.role, PinRole::Hook);
    assert!(rising.level);

    output.set_low();
    let falling = tokio::time::timeout(Duration::from_secs(1), port.next_edge()).await??;
    assert_eq!(falling.role, PinRole::Hook);
    assert!(!falling.level);

    Ok(())
}

fn loopback_output_bcm() -> u8 {
    std::env::var("PHONE_BOOTH_GPIO_LOOPBACK_OUT_BCM")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(23)
}
