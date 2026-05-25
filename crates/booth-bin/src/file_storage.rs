//! Filesystem-backed [`Storage`] implementation.
//!
//! Each key is stored as a file under a configurable base directory. Values are
//! written atomically (write-to-temp, then rename) so that a crash mid-write
//! cannot leave a corrupt key.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use booth_hal::{Storage, StorageError};

/// A [`Storage`] backend that persists each key as a file on disk.
///
/// Keys are percent-encoded to produce safe filenames. Writes use atomic
/// rename to avoid partial-write corruption.
pub struct FileStorage {
    base: PathBuf,
}

impl FileStorage {
    /// Create a new `FileStorage` rooted at `base`.
    ///
    /// The directory is created if it does not exist.
    pub fn new(base: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let base = base.into();
        std::fs::create_dir_all(&base).map_err(|err| {
            StorageError::Io(format!("create storage dir {}: {err}", base.display()).into())
        })?;
        Ok(Self { base })
    }

    fn key_path(&self, key: &str) -> PathBuf {
        self.base.join(encode_key(key))
    }

    fn temp_path(&self) -> PathBuf {
        self.base
            .join(format!(".tmp-{}-{}", std::process::id(), monotonic_ns()))
    }
}

#[async_trait]
impl Storage for FileStorage {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let path = self.key_path(key);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(StorageError::Io(
                format!("read {}: {err}", path.display()).into(),
            )),
        }
    }

    async fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        let final_path = self.key_path(key);
        let temp = self.temp_path();
        tokio::fs::write(&temp, value).await.map_err(|err| {
            StorageError::Io(format!("write temp {}: {err}", temp.display()).into())
        })?;
        tokio::fs::rename(&temp, &final_path).await.map_err(|err| {
            // Best-effort cleanup of the temp file.
            let _ = std::fs::remove_file(&temp);
            StorageError::Io(format!("rename to {}: {err}", final_path.display()).into())
        })?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let path = self.key_path(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) | Err(_) => Ok(()),
        }
    }
}

/// Percent-encode a key so it is safe as a filename.
///
/// Encodes `/`, `\`, `:`, and `%` as `%XX`.
fn encode_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for ch in key.chars() {
        match ch {
            '/' | '\\' | ':' | '%' | '\0' => {
                for byte in ch.to_string().as_bytes() {
                    out.push('%');
                    let _ = write!(out, "{byte:02X}");
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Decode a percent-encoded filename back to the original key.
#[cfg(test)]
fn decode_key(encoded: &str) -> String {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&encoded[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

fn monotonic_ns() -> u64 {
    use std::time::Instant;

    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    #[allow(clippy::cast_possible_truncation)]
    let ns = epoch.elapsed().as_nanos() as u64;
    ns
}

/// Return the base directory this storage writes to. Useful for tests.
impl FileStorage {
    /// Base directory path.
    pub fn base(&self) -> &Path {
        &self.base
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("file-storage-test-{}", monotonic_ns()));
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    #[tokio::test]
    async fn roundtrip() {
        let dir = temp_dir();
        let store = FileStorage::new(&dir).unwrap();
        store.set("hello", b"world").await.unwrap();
        let got = store.get("hello").await.unwrap();
        assert_eq!(got, Some(b"world".to_vec()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let dir = temp_dir();
        let store = FileStorage::new(&dir).unwrap();
        let got = store.get("nope").await.unwrap();
        assert_eq!(got, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn delete_removes_key() {
        let dir = temp_dir();
        let store = FileStorage::new(&dir).unwrap();
        store.set("key", b"val").await.unwrap();
        store.delete("key").await.unwrap();
        let got = store.get("key").await.unwrap();
        assert_eq!(got, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn keys_with_special_chars() {
        let dir = temp_dir();
        let store = FileStorage::new(&dir).unwrap();
        let key = "recording:abc123:path";
        store.set(key, b"/tmp/abc123.flac").await.unwrap();
        let got = store.get(key).await.unwrap();
        assert_eq!(got, Some(b"/tmp/abc123.flac".to_vec()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn encode_decode_roundtrip() {
        let keys = ["simple", "a/b/c", "recording:sha:path", "100%done", "a\\b"];
        for key in keys {
            let encoded = encode_key(key);
            let decoded = decode_key(&encoded);
            assert_eq!(decoded, key, "roundtrip failed for {key:?}");
        }
    }
}
