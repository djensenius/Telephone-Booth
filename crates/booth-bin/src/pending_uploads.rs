//! Durable pending-upload spool.
//!
//! Each recording awaiting upload is represented as a small JSON file in a spool
//! directory. The file is written **before** the upload attempt begins and
//! deleted only on confirmed success. On startup the directory is scanned to
//! discover uploads that were interrupted by a crash or restart.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Metadata stored in a spool entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpoolEntry {
    /// SHA-256 recording id (also the filename stem of the FLAC).
    pub recording_id: String,
    /// Operator question id associated with this recording, if any.
    pub question_id: Option<String>,
    /// Absolute path to the FLAC file on disk.
    pub path: String,
}

/// A handle to the pending-uploads spool directory.
pub struct PendingUploadSpool {
    dir: PathBuf,
}

impl PendingUploadSpool {
    /// Open (or create) the spool directory.
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Write a spool entry for a recording about to be uploaded.
    ///
    /// Uses atomic write (temp + rename) so a crash cannot leave a corrupt
    /// entry.
    pub fn enqueue(&self, entry: &SpoolEntry) -> std::io::Result<()> {
        let final_path = self.entry_path(&entry.recording_id);
        let temp_path = self
            .dir
            .join(format!(".tmp-{}-{}", std::process::id(), monotonic_ns()));
        let json = serde_json::to_vec(entry)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(&temp_path, &json)?;
        if let Err(err) = std::fs::rename(&temp_path, &final_path) {
            // Best-effort cleanup of the temp file on rename failure.
            let _ = std::fs::remove_file(&temp_path);
            return Err(err);
        }
        Ok(())
    }

    /// Remove a spool entry after a successful upload.
    pub fn dequeue(&self, recording_id: &str) -> std::io::Result<()> {
        let path = self.entry_path(recording_id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Scan the spool directory and return all pending entries.
    ///
    /// Entries that fail to parse are logged and skipped (they may be temp
    /// files from an interrupted write).
    pub fn scan(&self) -> Vec<SpoolEntry> {
        let mut entries = Vec::new();
        let read_dir = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(err) => {
                tracing::warn!(dir = %self.dir.display(), %err, "cannot read spool directory");
                return entries;
            }
        };
        for dir_entry in read_dir {
            let Ok(dir_entry) = dir_entry else { continue };
            let path = dir_entry.path();
            // Skip temp files and non-files.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            match std::fs::read(&path).and_then(|bytes| {
                serde_json::from_slice::<SpoolEntry>(&bytes)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
            }) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(), %err,
                        "skipping unparseable spool entry"
                    );
                }
            }
        }
        entries
    }

    /// The spool directory path.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry_path(&self, recording_id: &str) -> PathBuf {
        self.dir.join(recording_id)
    }
}

fn monotonic_ns() -> u64 {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Instant,
    };

    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    static LAST: AtomicU64 = AtomicU64::new(0);

    let epoch = EPOCH.get_or_init(Instant::now);
    let elapsed = u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX);

    loop {
        let last = LAST.load(Ordering::Relaxed);
        let next = elapsed.max(last.saturating_add(1));
        if LAST
            .compare_exchange(last, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return next;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spool-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn enqueue_and_scan() {
        let dir = temp_dir();
        let spool = PendingUploadSpool::open(&dir).unwrap();
        let entry = SpoolEntry {
            recording_id: "abc123".to_string(),
            question_id: Some("q42".to_string()),
            path: "/var/lib/phone-booth/recordings/abc123.flac".to_string(),
        };
        spool.enqueue(&entry).unwrap();

        let found = spool.scan();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], entry);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dequeue_removes_entry() {
        let dir = temp_dir();
        let spool = PendingUploadSpool::open(&dir).unwrap();
        let entry = SpoolEntry {
            recording_id: "def456".to_string(),
            question_id: None,
            path: "/tmp/def456.flac".to_string(),
        };
        spool.enqueue(&entry).unwrap();
        spool.dequeue("def456").unwrap();

        let found = spool.scan();
        assert!(found.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_skips_dotfiles() {
        let dir = temp_dir();
        let spool = PendingUploadSpool::open(&dir).unwrap();
        // Write a dotfile (simulating interrupted temp write)
        std::fs::write(dir.join(".tmp-123-456"), b"garbage").unwrap();
        let found = spool.scan();
        assert!(found.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn monotonic_ns_is_strictly_increasing() {
        let first = monotonic_ns();
        let second = monotonic_ns();

        assert!(second > first);
    }
}
