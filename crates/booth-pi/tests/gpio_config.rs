//! GPIO configuration regression tests.

#![allow(clippy::expect_used)]

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

// The Serde fallbacks and the Rust `Default` impl are independent code paths:
// an omitted `invert` table uses `GpioInvertConfig::default()`, while a present
// `invert` table with `rotary_pulse` omitted uses the field's
// `#[serde(default = ...)]`. Both must keep `rotary_pulse` inverted so the
// documented default can't silently drift back to `false`.

#[test]
fn deserializing_config_without_invert_table_defaults_rotary_pulse_inverted() {
    let config: GpioConfig = toml::from_str(
        r#"
hook_bcm = 17
rotary_pulse_bcm = 27
rotary_gate_bcm = 22
pull = "up"
debounce_ms = 25
"#,
    )
    .expect("config without invert table should deserialize");

    assert!(config.inverted(PinRole::RotaryPulse));
    assert!(!config.inverted(PinRole::RotaryRead));
    assert!(!config.inverted(PinRole::Hook));
}

#[test]
fn deserializing_invert_table_without_rotary_pulse_defaults_it_inverted() {
    let config: GpioConfig = toml::from_str(
        r#"
hook_bcm = 17
rotary_pulse_bcm = 27
rotary_gate_bcm = 22
pull = "up"
debounce_ms = 25

[invert]
hook = true
rotary_gate = true
"#,
    )
    .expect("invert table without rotary_pulse should deserialize");

    assert!(config.inverted(PinRole::RotaryPulse));
    assert!(config.inverted(PinRole::RotaryRead));
    assert!(config.inverted(PinRole::Hook));
}

#[test]
fn deserializing_invert_table_can_still_disable_rotary_pulse() {
    let config: GpioConfig = toml::from_str(
        r#"
hook_bcm = 17
rotary_pulse_bcm = 27
rotary_gate_bcm = 22
pull = "up"
debounce_ms = 25

[invert]
rotary_pulse = false
"#,
    )
    .expect("explicit rotary_pulse = false should deserialize");

    assert!(!config.inverted(PinRole::RotaryPulse));
}
