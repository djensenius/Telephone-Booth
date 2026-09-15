//! Durable event spool for the operator event forwarder.
//!
//! When the operator is unreachable and the in-memory buffer overflows,
//! failed batches are written to disk as numbered JSON files. On startup the
//! spool is scanned and replayed (oldest first) before new events flow.
//!
//! Events carry a stable `eventId` so the operator deduplicates replayed
//! batches — it is always safe to re-send.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;
use tracing::{debug, warn};

/// Maximum spool files retained on disk. Beyond this cap the oldest files
/// are deleted to bound disk usage (~200 events × 50 files ≈ 10k events).
const DEFAULT_MAX_FILES: usize = 50;

/// Handle to the event spool directory.
pub struct EventSpool {
    dir: PathBuf,
    max_files: usize,
    writer: Mutex<()>,
}

impl EventSpool {
    /// Open (or create) the spool directory.
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            max_files: DEFAULT_MAX_FILES,
            writer: Mutex::new(()),
        })
    }

    /// Write a failed batch to disk for later replay.
    pub fn spill(&self, batch: &[Value]) -> std::io::Result<()> {
        let _writer = self
            .writer
            .lock()
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        let sequence = self
            .entries()?
            .iter()
            .filter_map(|path| batch_sequence(path))
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("event spool sequence exhausted"))?;
        let filename = format!("batch-{sequence:020}.json");
        let path = self.dir.join(&filename);
        let tmp = self.dir.join(format!(".tmp-{filename}"));
        let body = serde_json::to_vec(batch)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        std::fs::File::open(&self.dir)?.sync_all()?;
        self.enforce_cap()?;
        Ok(())
    }

    /// Scan spool directory and return saved batches in oldest-first order.
    ///
    /// Each returned item is a `(path, body)` tuple: the path to the spool
    /// file and the ready-to-send JSON string (`{"events":[...]}`).
    ///
    /// Files are NOT deleted by this method — the caller must call
    /// [`Self::remove_file`] after each successful send. This prevents data
    /// loss if replay fails partway through.
    pub fn drain(&self) -> Vec<(PathBuf, String)> {
        let batches: Vec<_> = self
            .sorted_entries()
            .into_iter()
            .filter_map(Self::read_batch)
            .collect();
        debug!(count = batches.len(), "drained event spool");
        batches
    }

    pub(crate) fn oldest_batch(&self) -> Option<(PathBuf, String)> {
        self.sorted_entries()
            .into_iter()
            .next()
            .and_then(Self::read_batch)
    }

    fn read_batch(path: PathBuf) -> Option<(PathBuf, String)> {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                warn!(path = %path.display(), %err, "cannot read event spool file");
                return None;
            }
        };
        match serde_json::from_slice::<Vec<Value>>(&bytes) {
            Ok(events) => Some((path, serde_json::json!({ "events": events }).to_string())),
            Err(err) => {
                warn!(path = %path.display(), %err, "corrupt event spool file; removing");
                Self::remove_file(&path);
                None
            }
        }
    }

    /// Remove a single spool file after successful replay.
    pub fn remove_file(path: &Path) {
        if let Err(err) = std::fs::remove_file(path) {
            warn!(path = %path.display(), %err, "cannot remove event spool file");
        }
    }

    /// Returns `true` if there are spooled batches on disk.
    pub fn has_pending(&self) -> bool {
        !self.sorted_entries().is_empty()
    }

    fn sorted_entries(&self) -> Vec<PathBuf> {
        match self.entries() {
            Ok(paths) => paths,
            Err(err) => {
                warn!(path = %self.dir.display(), %err, "cannot scan event spool");
                Vec::new()
            }
        }
    }

    fn entries(&self) -> std::io::Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file()
                && path.extension().is_some_and(|ext| ext == "json")
                && !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
            {
                // Legacy process-relative names precede sequenced batches;
                // persisted modification times recover their cross-boot order.
                let key = match batch_sequence(&path) {
                    Some(sequence) => (true, u128::from(sequence)),
                    None => (
                        false,
                        entry
                            .metadata()?
                            .modified()?
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_err(std::io::Error::other)?
                            .as_nanos(),
                    ),
                };
                paths.push((key, path));
            }
        }
        paths.sort();
        Ok(paths.into_iter().map(|(_, path)| path).collect())
    }

    fn enforce_cap(&self) -> std::io::Result<()> {
        let entries = self.entries()?;
        if entries.len() > self.max_files {
            let to_remove = entries.len() - self.max_files;
            for path in entries.iter().take(to_remove) {
                std::fs::remove_file(path)?;
            }
            std::fs::File::open(&self.dir)?.sync_all()?;
        }
        Ok(())
    }
}

/// Resolve the event spool directory relative to a base data dir.
pub fn event_spool_dir_for(data_dir: &Path) -> PathBuf {
    data_dir.join("event-spool")
}

fn batch_sequence(path: &Path) -> Option<u64> {
    path.file_stem()?
        .to_str()?
        .strip_prefix("batch-")?
        .parse()
        .ok()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn restart_preserves_oldest_batch_and_retention_order() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("9999999999999999999-123.json");
        std::fs::write(&old, br#"[{"eventId":"old"}]"#).unwrap();
        let spool = EventSpool::open(dir.path()).unwrap();
        spool.spill(&[json!({"eventId": "new"})]).unwrap();
        assert_eq!(spool.oldest_batch().unwrap().0, old);
        drop(spool);

        let mut reopened = EventSpool::open(dir.path()).unwrap();
        reopened.max_files = 2;
        reopened.spill(&[json!({"eventId": "newest"})]).unwrap();
        let batches = reopened.drain();
        assert_eq!(batches.len(), 2);
        assert!(batches[0].1.contains("\"eventId\":\"new\""));
        assert!(batches[1].1.contains("\"eventId\":\"newest\""));
        assert!(!old.exists());
    }

    #[test]
    fn oldest_batch_does_not_read_later_files() {
        let dir = tempfile::tempdir().unwrap();
        let spool = EventSpool::open(dir.path()).unwrap();
        let first = dir.path().join("000.json");
        let later = dir.path().join("999.json");
        std::fs::write(&first, br#"[{"eventId":"first"}]"#).unwrap();
        std::fs::write(&later, b"invalid json").unwrap();
        assert_eq!(spool.oldest_batch().unwrap().0, first);
        assert!(
            later.exists(),
            "reading the corrupt later batch would remove it"
        );
    }

    #[test]
    fn round_trip_spill_and_drain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spool = EventSpool::open(dir.path()).expect("open");

        let batch = vec![
            json!({"eventId": "a", "type": "state_transition"}),
            json!({"eventId": "b", "type": "error"}),
        ];
        spool.spill(&batch).expect("spill");

        assert!(spool.has_pending());
        let drained = spool.drain();
        assert_eq!(drained.len(), 1);
        let (path, body) = &drained[0];
        assert!(body.contains("\"eventId\":\"a\""));
        // File still on disk until explicitly removed
        assert!(spool.has_pending());
        EventSpool::remove_file(path);
        assert!(!spool.has_pending());
    }

    #[test]
    fn enforces_max_file_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut spool = EventSpool::open(dir.path()).expect("open");
        spool.max_files = 3;

        for i in 0..5 {
            let batch = vec![json!({"eventId": format!("ev-{i}")})];
            spool.spill(&batch).expect("spill");
        }

        let entries = spool.sorted_entries();
        assert!(entries.len() <= 3);
    }
}
