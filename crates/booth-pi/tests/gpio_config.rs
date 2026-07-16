//! GPIO configuration regression tests.

use booth_hal::PinRole;
use booth_pi::{GpioConfig, GpioPull};

#[test]
fn default_gpio_config_matches_documented_wiring() {
    let config = GpioConfig::default();

    assert_eq!(config.bcm_for(PinRole::Hook), 17);
    assert_eq!(config.bcm_for(PinRole::RotaryPulse), 27);
    assert_eq!(config.bcm_for(PinRole::RotaryRead), 22);
    assert_eq!(config.pull, GpioPull::Up);
    assert_eq!(config.debounce_ms, 25);

    assert!(!config.inverted(PinRole::Hook));
    assert!(config.inverted(PinRole::RotaryPulse));
    assert!(!config.inverted(PinRole::RotaryRead));
}
