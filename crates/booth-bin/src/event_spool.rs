//! Durable event spool for the operator event forwarder.
//!
//! When the operator is unreachable and the in-memory buffer overflows,
//! failed batches are written to disk as numbered JSON files. On startup the
//! spool is scanned and replayed (oldest first) before new events flow.
//!
//! Events carry a stable `eventId` so the operator deduplicates replayed
//! batches — it is always safe to re-send.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{debug, warn};

/// Maximum spool files retained on disk. Beyond this cap the oldest files
/// are deleted to bound disk usage (~200 events × 50 files ≈ 10k events).
const DEFAULT_MAX_FILES: usize = 50;

/// Handle to the event spool directory.
pub struct EventSpool {
    dir: PathBuf,
    max_files: usize,
}

impl EventSpool {
    /// Open (or create) the spool directory.
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            max_files: DEFAULT_MAX_FILES,
        })
    }

    /// Write a failed batch to disk for later replay.
    pub fn spill(&self, batch: &[Value]) -> std::io::Result<()> {
        self.enforce_cap();
        let filename = format!("{}-{}.json", monotonic_ns(), std::process::id());
        let path = self.dir.join(&filename);
        let tmp = self.dir.join(format!(".tmp-{filename}"));
        let body = serde_json::to_vec(batch)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, &body)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Scan spool directory and return saved batches in oldest-first order.
    ///
    /// Each returned item is a ready-to-send JSON string (`{"events":[...]}`).
    pub fn drain(&self) -> Vec<String> {
        let mut entries = self.sorted_entries();
        let mut batches = Vec::with_capacity(entries.len());
        for path in &entries {
            match std::fs::read(path) {
                Ok(bytes) => {
                    // Wrap the raw event array into the envelope the operator expects.
                    match serde_json::from_slice::<Vec<Value>>(&bytes) {
                        Ok(events) => {
                            let envelope = serde_json::json!({ "events": events }).to_string();
                            batches.push(envelope);
                        }
                        Err(err) => {
                            warn!(path = %path.display(), %err, "corrupt event spool file; removing");
                        }
                    }
                }
                Err(err) => {
                    warn!(path = %path.display(), %err, "cannot read event spool file");
                }
            }
        }
        // Delete all scanned files; if replay fails the forwarder will
        // re-spill them.
        for path in &mut entries {
            let _ = std::fs::remove_file(path);
        }
        debug!(count = batches.len(), "drained event spool");
        batches
    }

    /// Returns `true` if there are spooled batches on disk.
    pub fn has_pending(&self) -> bool {
        !self.sorted_entries().is_empty()
    }

    fn sorted_entries(&self) -> Vec<PathBuf> {
        let Ok(read_dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = read_dir
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension().is_some_and(|ext| ext == "json")
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
            })
            .collect();
        paths.sort();
        paths
    }

    fn enforce_cap(&self) {
        let entries = self.sorted_entries();
        if entries.len() >= self.max_files {
            let to_remove = entries.len() - self.max_files + 1;
            for path in entries.iter().take(to_remove) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Resolve the event spool directory relative to a base data dir.
pub fn event_spool_dir_for(data_dir: &Path) -> PathBuf {
    data_dir.join("event-spool")
}

fn monotonic_ns() -> u64 {
    use std::time::Instant;

    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

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
        assert!(drained[0].contains("\"eventId\":\"a\""));
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
            // Small delay so filenames sort distinctly.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let entries = spool.sorted_entries();
        assert!(entries.len() <= 3);
    }
}
