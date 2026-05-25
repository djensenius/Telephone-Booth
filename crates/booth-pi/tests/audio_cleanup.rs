//! Tests for recording metadata cleanup in `PiAudioSource`.
//!
//! These run on any host (macOS or Linux) but require the `audio` feature so
//! that the recording code path is compiled. They are marked `ignore` because
//! they need real audio hardware to *start* a recording; the cleanup logic
//! itself is exercised via the storage layer.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use booth_hal::{AudioSource, RecordingId, Storage, StorageError};
use tokio::sync::Mutex;

use booth_pi::{AudioConfig, PiAudioSource};

/// Minimal in-memory storage for test assertions.
#[derive(Default, Clone)]
struct TestStorage {
    inner: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

#[async_trait::async_trait]
impl Storage for TestStorage {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.inner.lock().await.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        self.inner
            .lock()
            .await
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.inner.lock().await.remove(key);
        Ok(())
    }
}

/// After `cleanup_recording`, the recording id is no longer resolvable via
/// `path_of` and the storage key has been removed.
#[cfg(feature = "audio")]
#[tokio::test]
#[ignore = "requires audio hardware to produce a real recording"]
async fn cleanup_removes_finished_metadata() {
    let storage = TestStorage::default();
    let config = AudioConfig {
        max_recording_secs: 1,
        ..AudioConfig::default()
    };
    let mut source = PiAudioSource::new(config, Arc::new(storage.clone()));

    source.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let recording_id: RecordingId = source
        .stop()
        .await
        .unwrap()
        .expect("recording id should be present");

    // Metadata exists before cleanup.
    assert!(source.path_of(&recording_id).await.is_ok());
    let key = format!("recording:{recording_id}:path");
    assert!(storage.inner.lock().await.contains_key(&key));

    // After cleanup, metadata is gone.
    source.cleanup_recording(&recording_id).await.unwrap();
    assert!(source.path_of(&recording_id).await.is_err());
    assert!(!storage.inner.lock().await.contains_key(&key));
}

/// `cleanup_recording` on an unknown id succeeds without error (idempotent).
#[cfg(feature = "audio")]
#[tokio::test]
async fn cleanup_unknown_id_is_idempotent() {
    let storage = TestStorage::default();
    let config = AudioConfig::default();
    let source = PiAudioSource::new(config, Arc::new(storage));

    // Should not fail even though no recording with this id exists.
    let result = source.cleanup_recording(&"nonexistent".to_string()).await;
    assert!(result.is_ok());
}
