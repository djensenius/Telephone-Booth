//! Host-runnable tests for Pi audio configuration and bundled tone assets.

use booth_hal::BuiltinTone;
use booth_pi::{AudioConfig, device_name_matches, embedded_tone_bytes, has_flac_stream_marker};

#[test]
fn audio_config_defaults_match_hardware_docs() {
    let config = AudioConfig::default();

    assert_eq!(config.device_substring.as_deref(), Some("Focusrite"));
    assert_eq!(config.sample_rate_hz, 48_000);
    assert_eq!(config.channels, 1);
    assert_eq!(config.max_recording_secs, 60);
    assert_eq!(config.recordings_dir, "/var/lib/phone-booth/recordings");
}

#[test]
fn device_matching_is_case_insensitive_and_ignores_empty_needles() {
    assert!(device_name_matches(
        "Scarlett 2i2 USB Focusrite",
        Some("focusRITE")
    ));
    assert!(!device_name_matches("Built-in Output", Some("Focusrite")));
    assert!(!device_name_matches("Built-in Output", None));
    assert!(!device_name_matches("Built-in Output", Some("  ")));
}

#[test]
fn embedded_tones_are_flac_streams() {
    for tone in [
        BuiltinTone::DialTone,
        BuiltinTone::Beep,
        BuiltinTone::LineBusy,
        BuiltinTone::CallUnavailable,
    ] {
        let bytes = match embedded_tone_bytes(tone) {
            Ok(bytes) => bytes,
            Err(err) => panic!("tone should be bundled: {err}"),
        };
        assert!(has_flac_stream_marker(bytes));
    }
}

#[test]
fn all_builtin_tones_are_bundled() {
    assert!(embedded_tone_bytes(BuiltinTone::LineBusy).is_ok());
}

#[cfg(feature = "pi")]
#[test]
#[ignore = "requires Raspberry Pi audio hardware"]
fn enumerates_pi_audio_devices() -> Result<(), Box<dyn std::error::Error>> {
    use cpal::traits::HostTrait;

    let host = cpal::default_host();
    let input_count = host.input_devices()?.count();
    let output_count = host.output_devices()?.count();

    assert!(input_count > 0);
    assert!(output_count > 0);
    Ok(())
}

#[cfg(feature = "pi")]
#[tokio::test]
#[ignore = "requires a physical loopback cable between input and output"]
async fn one_second_loopback_record_then_playback()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::Arc;

    use booth_hal::{AudioRef, AudioSink, AudioSource, Storage, StorageError};
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct MemoryStorage(Mutex<std::collections::HashMap<String, Vec<u8>>>);

    #[async_trait::async_trait]
    impl Storage for MemoryStorage {
        async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(self.0.lock().await.get(key).cloned())
        }

        async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
            self.0.lock().await.insert(key.to_string(), value.to_vec());
            Ok(())
        }

        async fn delete(&self, key: &str) -> Result<(), StorageError> {
            self.0.lock().await.remove(key);
            Ok(())
        }
    }

    let config = AudioConfig {
        max_recording_secs: 1,
        ..AudioConfig::default()
    };
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let mut source = booth_pi::PiAudioSource::new(config.clone(), storage);
    let mut sink = booth_pi::PiAudioSink::new(config);

    source.start().await?;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let recording_id = source
        .stop()
        .await?
        .ok_or_else(|| std::io::Error::other("recording id missing"))?;
    let path = source.path_of(&recording_id).await?;
    sink.play(AudioRef::LocalFile(path)).await?;
    sink.wait_for_end().await?;
    Ok(())
}
