//! Startup ALSA mixer configuration.
//!
//! Applies [`crate::MixerConfig`] once when the Pi runtime builds its adapters, so
//! hardware capture/playback gain and on/off switches are set by the
//! application rather than depending on `alsactl store` / `alsa-restore`
//! persisting across reboots.
//!
//! The real implementation is compiled only on Linux with the `audio` feature
//! (where the `alsa` crate and `libasound` are available) — this includes the
//! Linux cross-compile targets. On every other target (notably the macOS host
//! build) it degrades to a logged no-op so the workspace still type-checks.

use crate::AudioConfig;

/// Errors raised while applying the startup mixer configuration.
#[derive(Debug, thiserror::Error)]
pub enum MixerError {
    /// The ALSA mixer for the configured card could not be opened.
    #[error("open ALSA mixer '{device}': {message}")]
    Open {
        /// The control device name that failed to open.
        device: String,
        /// Underlying ALSA error text.
        message: String,
    },
}

/// Convert a `0`–`100` percentage into an absolute value within `min..=max`.
///
/// Percentages above `100` are clamped. The result is rounded toward zero,
/// matching `amixer`'s integer volume mapping.
#[must_use]
pub fn volume_from_percent(min: i64, max: i64, percent: u8) -> i64 {
    let pct = i64::from(percent.min(100));
    min + (max - min) * pct / 100
}

/// Normalize a configured card identifier into an ALSA control device name.
///
/// * A pure integer (`"1"`) becomes `"hw:1"`.
/// * A value already containing `':'` (`"hw:1"`, `"default"`, `"hw:CARD=Device"`)
///   is used verbatim.
/// * Any other bare id (`"Device"`) becomes `"hw:CARD=Device"`.
/// * `None`/empty, or the literal `"default"`, falls back to `"default"`.
#[must_use]
pub fn resolve_mixer_device(card: Option<&str>) -> String {
    match card.map(str::trim).filter(|c| !c.is_empty()) {
        None => "default".to_string(),
        Some(c) if c.eq_ignore_ascii_case("default") => "default".to_string(),
        Some(c) if c.contains(':') => c.to_string(),
        Some(c) if c.chars().all(|ch| ch.is_ascii_digit()) => format!("hw:{c}"),
        Some(c) => format!("hw:CARD={c}"),
    }
}

/// Apply the startup mixer settings described by `config.mixer`, if any.
///
/// Best-effort: opening the mixer failing is returned as [`MixerError`] for the
/// caller to log, but individual controls that are missing or reject a value
/// are logged as warnings and skipped so one bad entry never aborts the rest.
///
/// # Errors
///
/// Returns [`MixerError::Open`] when the ALSA mixer for the configured card
/// cannot be opened.
#[cfg(all(feature = "audio", target_os = "linux"))]
pub fn apply_startup_mixer(config: &AudioConfig) -> Result<(), MixerError> {
    use alsa::mixer::{Mixer, SelemId};
    use tracing::{info, warn};

    let Some(mixer_cfg) = config.mixer.as_ref() else {
        return Ok(());
    };
    if mixer_cfg.controls.is_empty() {
        return Ok(());
    }

    let device = resolve_mixer_device(mixer_cfg.card.as_deref());
    let mixer = Mixer::new(&device, false).map_err(|err| MixerError::Open {
        device: device.clone(),
        message: err.to_string(),
    })?;

    for control in &mixer_cfg.controls {
        let sid = SelemId::new(&control.name, control.index);
        let Some(selem) = mixer.find_selem(&sid) else {
            warn!(
                control = control.name.as_str(),
                index = control.index,
                device = device.as_str(),
                "mixer control not found; skipping"
            );
            continue;
        };
        apply_control(&selem, control);
    }

    info!(
        device = device.as_str(),
        controls = mixer_cfg.controls.len(),
        "applied startup ALSA mixer settings"
    );
    Ok(())
}

/// Apply a single resolved [`crate::MixerControl`] to its ALSA `Selem`.
///
/// Each requested aspect is applied independently; a failure on one is logged
/// and skipped so the remaining aspects (and controls) still run.
#[cfg(all(feature = "audio", target_os = "linux"))]
fn apply_control(selem: &alsa::mixer::Selem<'_>, control: &crate::MixerControl) {
    use tracing::warn;

    let name = control.name.as_str();
    if let Some(pct) = control.playback_volume_percent {
        let (min, max) = selem.get_playback_volume_range();
        if let Err(err) = selem.set_playback_volume_all(volume_from_percent(min, max, pct)) {
            warn!(control = name, %err, "set playback volume failed");
        }
    }
    if let Some(pct) = control.capture_volume_percent {
        let (min, max) = selem.get_capture_volume_range();
        if let Err(err) = selem.set_capture_volume_all(volume_from_percent(min, max, pct)) {
            warn!(control = name, %err, "set capture volume failed");
        }
    }
    if let Some(on) = control.playback_switch
        && let Err(err) = selem.set_playback_switch_all(i32::from(on))
    {
        warn!(control = name, %err, "set playback switch failed");
    }
    if let Some(on) = control.capture_switch
        && let Err(err) = selem.set_capture_switch_all(i32::from(on))
    {
        warn!(control = name, %err, "set capture switch failed");
    }
    if let Some(on) = control.switch {
        let value = i32::from(on);
        let has_playback = selem.has_playback_switch();
        let has_capture = selem.has_capture_switch();
        if has_playback && let Err(err) = selem.set_playback_switch_all(value) {
            warn!(control = name, %err, "set switch (playback) failed");
        }
        if has_capture && let Err(err) = selem.set_capture_switch_all(value) {
            warn!(control = name, %err, "set switch (capture) failed");
        }
        if !has_playback && !has_capture {
            warn!(
                control = name,
                "control has no playback/capture switch; `switch` ignored"
            );
        }
    }
}

/// No-op fallback for non-Linux or non-`audio` builds.
///
/// # Errors
///
/// Never returns an error on this platform; the signature mirrors the Linux
/// implementation so callers are platform-agnostic.
#[cfg(not(all(feature = "audio", target_os = "linux")))]
pub fn apply_startup_mixer(config: &AudioConfig) -> Result<(), MixerError> {
    if config
        .mixer
        .as_ref()
        .is_some_and(|mixer| !mixer.controls.is_empty())
    {
        tracing::debug!(
            "mixer settings are configured but ALSA mixer control is unavailable on this build; skipping"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use crate::MixerControl;

    #[test]
    fn volume_percent_maps_range() {
        assert_eq!(volume_from_percent(0, 100, 0), 0);
        assert_eq!(volume_from_percent(0, 100, 50), 50);
        assert_eq!(volume_from_percent(0, 100, 100), 100);
    }

    #[test]
    fn volume_percent_handles_nonzero_min_and_clamps() {
        // Matches the dongle's Mic range (0..=35): ~83% -> 29.
        assert_eq!(volume_from_percent(0, 35, 83), 29);
        // Negative-min ranges (dB-scaled controls) still interpolate.
        assert_eq!(volume_from_percent(-40, 0, 50), -20);
        // Above 100 clamps to max.
        assert_eq!(volume_from_percent(0, 35, 200), 35);
    }

    #[test]
    fn resolve_device_variants() {
        assert_eq!(resolve_mixer_device(None), "default");
        assert_eq!(resolve_mixer_device(Some("  ")), "default");
        assert_eq!(resolve_mixer_device(Some("1")), "hw:1");
        assert_eq!(resolve_mixer_device(Some("Device")), "hw:CARD=Device");
        assert_eq!(resolve_mixer_device(Some("hw:1")), "hw:1");
        assert_eq!(resolve_mixer_device(Some("default")), "default");
        assert_eq!(
            resolve_mixer_device(Some("hw:CARD=Device")),
            "hw:CARD=Device"
        );
    }

    #[test]
    fn apply_is_noop_without_mixer_config() {
        let config = AudioConfig::default();
        assert!(apply_startup_mixer(&config).is_ok());
    }

    #[test]
    fn mixer_config_deserializes_from_toml() {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            audio: AudioConfig,
        }

        let toml = r#"
            [audio]
            device_substring = "plughw:CARD=Device"

            [audio.mixer]
            card = "Device"

            [[audio.mixer.controls]]
            name = "Mic"
            capture_volume_percent = 83

            [[audio.mixer.controls]]
            name = "Speaker"
            playback_volume_percent = 100

            [[audio.mixer.controls]]
            name = "Auto Gain Control"
            switch = false
        "#;

        let parsed: Wrapper = toml::from_str(toml).expect("parse mixer config");
        let mixer = parsed.audio.mixer.expect("mixer present");
        assert_eq!(mixer.card.as_deref(), Some("Device"));
        assert_eq!(
            mixer.controls,
            vec![
                MixerControl {
                    name: "Mic".to_string(),
                    capture_volume_percent: Some(83),
                    ..MixerControl::default()
                },
                MixerControl {
                    name: "Speaker".to_string(),
                    playback_volume_percent: Some(100),
                    ..MixerControl::default()
                },
                MixerControl {
                    name: "Auto Gain Control".to_string(),
                    switch: Some(false),
                    ..MixerControl::default()
                },
            ]
        );
    }
}
