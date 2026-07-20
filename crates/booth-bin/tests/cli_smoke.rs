//! CLI smoke tests for the `telephone-booth` binary.

use std::error::Error;
use std::process::Command;

#[test]
fn print_config_exits_zero_and_emits_toml() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_telephone-booth"))
        .args(["print-config", "--config", "/dev/null"])
        .output()?;

    assert!(output.status.success(), "print-config failed: {output:?}");
    let stdout = String::from_utf8(output.stdout)?;
    let parsed: toml::Value = toml::from_str(&stdout)?;
    assert!(parsed.get("gpio").is_some());
    assert!(parsed.get("audio").is_some());
    assert!(parsed.get("operator").is_some());
    Ok(())
}

#[test]
fn check_dev_null_exits_nonzero() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_telephone-booth"))
        .args(["check", "--config", "/dev/null"])
        .output()?;

    assert!(!output.status.success(), "check unexpectedly succeeded");
    Ok(())
}

#[test]
fn simulate_five_pulses_prints_expected_effects() -> Result<(), Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_telephone-booth"))
        .args(["simulate", "5"])
        .output()?;

    assert!(output.status.success(), "simulate failed: {output:?}");
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("CallUnavailable"));
    assert!(stdout.contains("Builtin(CallUnavailable)"));
    Ok(())
}
