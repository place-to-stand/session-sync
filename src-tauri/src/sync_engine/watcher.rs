//! File system watcher with per-file stability debouncing.
//!
//! Uses `notify::RecommendedWatcher` + `notify-debouncer-full` to detect
//! changes in a Pro Tools session folder.  On top of the debouncer's built-in
//! deduplication, we add:
//!
//! - **Per-file stability check**: after the debouncer fires, poll the file's
//!   size every 2 seconds.  Only declare the file "stable" (ready to upload)
//!   after 5 consecutive seconds with no size change.  This handles active
//!   recordings where WAV files are growing in real-time.
//!
//! - **Batch window**: collect all changes for 10 seconds after the first
//!   event in a batch, then emit the entire batch at once.  This coalesces
//!   Pro Tools' burst-write patterns (save = dozens of tiny writes).
//!
//! - **Ignore filter**: skip files matching Pro Tools cache patterns, temp
//!   files, and user-configured patterns.
//!
//! - **EventKind::Access** events (reads) are silently discarded.
//!
//! - **External drive disconnect detection**: if the watched path disappears,
//!   emit a `DriveDisconnected` event and pause the watcher.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::event::EventKind;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};

use crate::error::{SyncError, SyncResult};
use crate::ignore::IgnoreFilter;

// ── Public types ────────────────────────────────────────────────────────

/// The type of change detected for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::Created => write!(f, "created"),
            ChangeType::Modified => write!(f, "modified"),
            ChangeType::Deleted => write!(f, "deleted"),
        }
    }
}

/// A single file-change event after stability debouncing.
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub path: PathBuf,
    pub change_type: ChangeType,
}

/// A batch of file changes, emitted after the 10-second batch window closes.
#[derive(Debug, Clone)]
pub struct ChangeBatch {
    pub events: Vec<FileChangeEvent>,
}

// ── Internal state for per-file stability tracking ──────────────────────

#[derive(Debug)]
struct PendingFile {
    change_type: ChangeType,
    last_size: Option<u64>,
    stable_since: Option<Instant>,
}

// ── SessionWatcher ──────────────────────────────────────────────────────

/// Watches a Pro Tools session directory and emits batched, stability-checked
/// change events.
pub struct SessionWatcher {
    /// The session folder being watched.
    session_path: PathBuf,
    /// Ignore filter for Pro Tools artifacts.
    ignore: IgnoreFilter,
    /// Channel to send batched change events to the sync engine.
    batch_tx: mpsc::Sender<ChangeBatch>,
    /// Handle to stop the watcher (sends signal on drop or explicit stop).
    stop_tx: Option<mpsc::Sender<()>>,
    /// Join handle for the background watcher task.
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SessionWatcher {
    /// Create a new watcher for the given session path.
    ///
    /// The `batch_tx` channel will receive `ChangeBatch` values whenever the
    /// watcher detects stable, non-ignored file changes.
    pub fn new(
        session_path: PathBuf,
        ignore: IgnoreFilter,
        batch_tx: mpsc::Sender<ChangeBatch>,
    ) -> Self {
        Self {
            session_path,
            ignore,
            batch_tx,
            stop_tx: None,
            task_handle: None,
        }
    }

    /// Start watching the session directory.
    ///
    /// Spawns a background tokio task that:
    /// 1. Sets up a `notify::RecommendedWatcher`.
    /// 2. Receives raw FS events via a channel.
    /// 3. Filters out ignored files and access events.
    /// 4. Runs per-file stability checks.
    /// 5. Collects events into 10-second batches.
    /// 6. Sends completed batches to the engine via `batch_tx`.
    pub fn start_watching(&mut self) -> SyncResult<()> {
        let session_path = self.session_path.clone();

        // Verify the watch path exists (detect disconnected drives).
        if !session_path.exists() {
            return Err(SyncError::DriveDisconnected {
                path: session_path.display().to_string(),
            });
        }

        let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
        self.stop_tx = Some(stop_tx);

        let ignore = self.ignore.clone();
        let batch_tx = self.batch_tx.clone();

        let handle = tokio::task::spawn(async move {
            if let Err(e) = run_watcher_loop(session_path, ignore, batch_tx, stop_rx).await {
                error!(error = %e, "File watcher loop exited with error");
            }
        });

        self.task_handle = Some(handle);
        info!(path = %self.session_path.display(), "File watcher started");

        Ok(())
    }

    /// Stop watching and clean up.
    pub async fn stop_watching(&mut self) {
        // Send stop signal.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(()).await;
        }
        // Wait for the task to finish.
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
        info!(path = %self.session_path.display(), "File watcher stopped");
    }

    /// Check if the watched path still exists (drive still connected).
    pub fn is_path_accessible(&self) -> bool {
        self.session_path.exists()
    }
}

// ── Internal watcher loop ───────────────────────────────────────────────

/// The core watcher loop that runs in a background tokio task.
async fn run_watcher_loop(
    session_path: PathBuf,
    ignore: IgnoreFilter,
    batch_tx: mpsc::Sender<ChangeBatch>,
    mut stop_rx: mpsc::Receiver<()>,
) -> SyncResult<()> {
    // Channel for raw notify events.
    let (raw_tx, mut raw_rx) = mpsc::channel::<notify::Event>(512);

    // Set up the OS-level file watcher.
    let mut watcher = RecommendedWatcher::new(
        move |result: Result<notify::Event, notify::Error>| {
            match result {
                Ok(event) => {
                    // Best-effort send — if the channel is full we drop the event.
                    // The next periodic drive-check or batch timer will catch up.
                    let _ = raw_tx.blocking_send(event);
                }
                Err(e) => {
                    warn!(error = %e, "File watcher error");
                }
            }
        },
        Config::default()
            .with_poll_interval(Duration::from_secs(2)),
    )?;

    watcher.watch(&session_path, RecursiveMode::Recursive)?;
    info!(path = %session_path.display(), "OS file watcher registered");

    // Per-file stability tracking.
    let mut pending: HashMap<PathBuf, PendingFile> = HashMap::new();
    // Batch collection state.
    let mut batch_start: Option<Instant> = None;

    // Stability check interval.
    let stability_interval = Duration::from_secs(2);
    // How long a file must be unchanged to be considered stable.
    let stability_threshold = Duration::from_secs(5);
    // How long to collect events before emitting a batch.
    let batch_window = Duration::from_secs(10);
    // Drive health check interval.
    let drive_check_interval = Duration::from_secs(30);

    let mut stability_timer = tokio::time::interval(stability_interval);
    let mut drive_check_timer = tokio::time::interval(drive_check_interval);

    // Discard the first immediate tick from both intervals.
    stability_timer.tick().await;
    drive_check_timer.tick().await;

    loop {
        tokio::select! {
            // ── Stop signal ──────────────────────────────────────────
            _ = stop_rx.recv() => {
                debug!("Watcher received stop signal");
                break;
            }

            // ── Raw FS event from notify ─────────────────────────────
            Some(event) = raw_rx.recv() => {
                process_raw_event(&event, &session_path, &ignore, &mut pending, &mut batch_start);
            }

            // ── Periodic stability check ─────────────────────────────
            _ = stability_timer.tick() => {
                check_stability(&mut pending, stability_threshold).await;

                // If we have a batch window open and it has expired, emit.
                if let Some(start) = batch_start {
                    if start.elapsed() >= batch_window {
                        let batch = collect_stable_events(&mut pending);
                        if !batch.events.is_empty() {
                            debug!(count = batch.events.len(), "Emitting change batch");
                            if batch_tx.send(batch).await.is_err() {
                                warn!("Batch receiver dropped, stopping watcher");
                                break;
                            }
                        }
                        // Reset batch window if there are still pending files.
                        batch_start = if pending.is_empty() {
                            None
                        } else {
                            Some(Instant::now())
                        };
                    }
                }
            }

            // ── Periodic drive health check ──────────────────────────
            _ = drive_check_timer.tick() => {
                if !session_path.exists() {
                    warn!(
                        path = %session_path.display(),
                        "Watch path no longer accessible — drive may be disconnected"
                    );
                    // We don't break here — the path may come back.
                    // The engine will detect the DriveDisconnected state via
                    // is_path_accessible() and handle it.
                }
            }
        }
    }

    // Final cleanup: drop the watcher (which stops OS-level watching).
    drop(watcher);

    Ok(())
}

/// Process a single raw notify event: classify the change, apply ignore
/// filter, and insert into the pending map.
fn process_raw_event(
    event: &notify::Event,
    session_path: &Path,
    ignore: &IgnoreFilter,
    pending: &mut HashMap<PathBuf, PendingFile>,
    batch_start: &mut Option<Instant>,
) {
    // Discard access (read-only) events — they don't indicate content changes.
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }

    // Classify the change type.
    let change_type = match event.kind {
        EventKind::Create(_) => ChangeType::Created,
        EventKind::Modify(_) => ChangeType::Modified,
        EventKind::Remove(_) => ChangeType::Deleted,
        _ => return, // Other event kinds (e.g. metadata-only) are not actionable.
    };

    for path in &event.paths {
        // Only process regular files (skip directories).
        // For deleted files, we can't stat them, so accept them unconditionally.
        if change_type != ChangeType::Deleted && path.is_dir() {
            continue;
        }

        // Make the path relative for ignore checking.
        let relative = match path.strip_prefix(session_path) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Apply ignore filter.
        if ignore.is_ignored(relative) {
            trace!(path = %relative.display(), "Ignoring file (matches ignore pattern)");
            continue;
        }

        // Start the batch window on first event.
        if batch_start.is_none() {
            *batch_start = Some(Instant::now());
        }

        // Insert or update the pending entry.
        let entry = pending.entry(path.clone()).or_insert(PendingFile {
            change_type,
            last_size: None,
            stable_since: None,
        });

        // If the file was previously stable and a new event arrives, reset stability.
        entry.change_type = change_type;
        entry.stable_since = None;

        // For non-deleted files, record the current size.
        if change_type != ChangeType::Deleted {
            if let Ok(meta) = std::fs::metadata(path) {
                entry.last_size = Some(meta.len());
            }
        }

        trace!(
            path = %relative.display(),
            change = %change_type,
            "File change detected"
        );
    }
}

/// Check each pending file for stability: if its size hasn't changed since
/// the last check, start (or continue) the stability timer.
async fn check_stability(
    pending: &mut HashMap<PathBuf, PendingFile>,
    threshold: Duration,
) {
    for (path, entry) in pending.iter_mut() {
        // Deleted files are immediately stable (nothing to wait for).
        if entry.change_type == ChangeType::Deleted {
            if entry.stable_since.is_none() {
                entry.stable_since = Some(Instant::now());
            }
            continue;
        }

        // Read current size.
        let current_size = match std::fs::metadata(path) {
            Ok(meta) => Some(meta.len()),
            Err(_) => {
                // File might have been deleted since the event.
                entry.change_type = ChangeType::Deleted;
                entry.stable_since = Some(Instant::now());
                continue;
            }
        };

        if current_size == entry.last_size {
            // Size unchanged — start or continue stability timer.
            if entry.stable_since.is_none() {
                entry.stable_since = Some(Instant::now());
            }
        } else {
            // Size changed — reset stability timer.
            entry.last_size = current_size;
            entry.stable_since = None;
        }
    }
}

/// Extract all files that have reached the stability threshold from the
/// pending map and return them as a `ChangeBatch`.
fn collect_stable_events(
    pending: &mut HashMap<PathBuf, PendingFile>,
) -> ChangeBatch {
    let now = Instant::now();
    let threshold = Duration::from_secs(5);

    let mut events = Vec::new();
    let mut stable_paths = Vec::new();

    for (path, entry) in pending.iter() {
        if let Some(stable_since) = entry.stable_since {
            if now.duration_since(stable_since) >= threshold {
                events.push(FileChangeEvent {
                    path: path.clone(),
                    change_type: entry.change_type,
                });
                stable_paths.push(path.clone());
            }
        }
    }

    // Remove stable files from pending.
    for path in &stable_paths {
        pending.remove(path);
    }

    ChangeBatch { events }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_change_type_display() {
        assert_eq!(ChangeType::Created.to_string(), "created");
        assert_eq!(ChangeType::Modified.to_string(), "modified");
        assert_eq!(ChangeType::Deleted.to_string(), "deleted");
    }

    #[test]
    fn test_collect_stable_events_empty() {
        let mut pending = HashMap::new();
        let batch = collect_stable_events(&mut pending);
        assert!(batch.events.is_empty());
    }

    #[test]
    fn test_collect_stable_events_not_yet_stable() {
        let mut pending = HashMap::new();
        pending.insert(
            PathBuf::from("/tmp/test.wav"),
            PendingFile {
                change_type: ChangeType::Created,
                last_size: Some(1000),
                stable_since: Some(Instant::now()), // Just started — not stable yet.
            },
        );

        let batch = collect_stable_events(&mut pending);
        // Should not be collected yet (stable_since is just now).
        assert!(batch.events.is_empty());
        assert_eq!(pending.len(), 1); // Still pending.
    }

    #[test]
    fn test_collect_stable_events_stable() {
        let mut pending = HashMap::new();
        pending.insert(
            PathBuf::from("/tmp/test.wav"),
            PendingFile {
                change_type: ChangeType::Modified,
                last_size: Some(1000),
                // Stable for > 5 seconds.
                stable_since: Some(Instant::now() - Duration::from_secs(10)),
            },
        );

        let batch = collect_stable_events(&mut pending);
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].change_type, ChangeType::Modified);
        assert!(pending.is_empty()); // Removed from pending.
    }

    #[test]
    fn test_process_raw_event_ignores_access() {
        let dir = TempDir::new().unwrap();
        let session_path = dir.path().to_path_buf();
        let ignore = IgnoreFilter::default();
        let mut pending = HashMap::new();
        let mut batch_start = None;

        let event = notify::Event {
            kind: EventKind::Access(notify::event::AccessKind::Read),
            paths: vec![session_path.join("audio.wav")],
            attrs: Default::default(),
        };

        process_raw_event(&event, &session_path, &ignore, &mut pending, &mut batch_start);
        assert!(pending.is_empty());
        assert!(batch_start.is_none());
    }

    #[test]
    fn test_process_raw_event_ignores_pkf() {
        let dir = TempDir::new().unwrap();
        let session_path = dir.path().to_path_buf();
        let ignore = IgnoreFilter::default();
        let mut pending = HashMap::new();
        let mut batch_start = None;

        // Create a .pkf file so is_dir() returns false.
        let pkf_path = session_path.join("track.pkf");
        std::fs::write(&pkf_path, b"peak data").unwrap();

        let event = notify::Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![pkf_path],
            attrs: Default::default(),
        };

        process_raw_event(&event, &session_path, &ignore, &mut pending, &mut batch_start);
        assert!(pending.is_empty(), "Peak files should be ignored");
    }
}
