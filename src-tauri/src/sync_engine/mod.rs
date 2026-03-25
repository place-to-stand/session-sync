//! Sync engine orchestrator.
//!
//! The `SyncEngine` manages the complete lifecycle of a single session's
//! sync operations:
//!
//! 1. **File watching** -- detect changes in the session folder.
//! 2. **Scanning & hashing** -- compute BLAKE3 hashes, diff against SQLite.
//! 3. **Auto-push** -- upload changed files to R2 via presigned URLs.
//! 4. **Version snapshots** -- create manifest every 5 minutes.
//! 5. **Heartbeat** -- send keepalive to Convex every 5 minutes.
//! 6. **Checkout/release** -- coordinate session ownership via Convex.
//! 7. **WAL replay** -- recover from crashes on startup.
//!
//! The engine runs as a set of cooperating tokio tasks controlled by an
//! internal command channel. The `SyncEngineHandle` provides a clonable,
//! Send + Sync interface for IPC commands to drive the engine.

pub mod checkout;
pub mod hasher;
pub mod scanner;
pub mod versioner;
pub mod wal;
pub mod watcher;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::error::{SyncError, SyncResult};
use crate::ignore::IgnoreFilter;
use crate::state::db::Database;
use crate::state::models::*;

use checkout::{CheckoutManager, ConvexClient};
use scanner::FileEntry;
use wal::WriteAheadLog;
use watcher::{ChangeBatch, SessionWatcher};

// ── Engine state machine ────────────────────────────────────────────────

/// The engine cycles through these states while managing a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    /// Engine created but not yet started (or between push cycles).
    Idle,
    /// File watcher is active, monitoring for changes.
    Watching,
    /// Actively uploading changed files to R2.
    Pushing,
    /// Downloading files from R2 (pull operation).
    Pulling,
    /// Engine is shutting down gracefully.
    ShuttingDown,
    /// Engine has stopped.
    Stopped,
}

impl std::fmt::Display for EngineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineState::Idle => write!(f, "idle"),
            EngineState::Watching => write!(f, "watching"),
            EngineState::Pushing => write!(f, "pushing"),
            EngineState::Pulling => write!(f, "pulling"),
            EngineState::ShuttingDown => write!(f, "shutting_down"),
            EngineState::Stopped => write!(f, "stopped"),
        }
    }
}

// ── Commands (sent from IPC -> engine) ──────────────────────────────────

/// Commands that external code (Tauri IPC handlers) can send to a running
/// engine instance via the `SyncEngineHandle`.
#[derive(Debug)]
pub enum EngineCommand {
    /// Check out the session (acquire the Convex lock, start watching).
    Checkout {
        reply: oneshot::Sender<SyncResult<()>>,
    },
    /// Release the session (create final version, release Convex lock, stop watching).
    Release {
        note: Option<String>,
        reply: oneshot::Sender<SyncResult<()>>,
    },
    /// Pull a specific version (or latest) from R2.
    Pull {
        version: Option<u64>,
        reply: oneshot::Sender<SyncResult<()>>,
    },
    /// Cancel an in-progress operation.
    Cancel,
    /// Gracefully shut down the engine.
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

// ── SyncEngineHandle ────────────────────────────────────────────────────

/// Clonable, Send + Sync handle for controlling a running `SyncEngine`.
///
/// This is the interface that Tauri IPC command handlers use to interact
/// with the engine.  It sends commands over a tokio mpsc channel to the
/// engine's run loop.
#[derive(Clone)]
pub struct SyncEngineHandle {
    cmd_tx: mpsc::Sender<EngineCommand>,
    state_rx: watch::Receiver<EngineState>,
    session_id: String,
}

impl SyncEngineHandle {
    /// Get the current engine state without blocking.
    pub fn state(&self) -> EngineState {
        *self.state_rx.borrow()
    }

    /// Get the session ID this engine manages.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Request checkout of the session.
    pub async fn checkout(&self) -> SyncResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::Checkout { reply: reply_tx })
            .await
            .map_err(|_| SyncError::Other("Engine channel closed".to_string()))?;
        reply_rx
            .await
            .map_err(|_| SyncError::Other("Engine dropped reply channel".to_string()))?
    }

    /// Release the session with an optional note.
    pub async fn release(&self, note: Option<String>) -> SyncResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::Release {
                note,
                reply: reply_tx,
            })
            .await
            .map_err(|_| SyncError::Other("Engine channel closed".to_string()))?;
        reply_rx
            .await
            .map_err(|_| SyncError::Other("Engine dropped reply channel".to_string()))?
    }

    /// Pull a version from R2 (None = latest released).
    pub async fn pull(&self, version: Option<u64>) -> SyncResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::Pull {
                version,
                reply: reply_tx,
            })
            .await
            .map_err(|_| SyncError::Other("Engine channel closed".to_string()))?;
        reply_rx
            .await
            .map_err(|_| SyncError::Other("Engine dropped reply channel".to_string()))?
    }

    /// Cancel the current operation.
    pub async fn cancel(&self) -> SyncResult<()> {
        self.cmd_tx
            .send(EngineCommand::Cancel)
            .await
            .map_err(|_| SyncError::Other("Engine channel closed".to_string()))?;
        Ok(())
    }

    /// Gracefully shut down the engine and wait for it to stop.
    pub async fn shutdown(&self) -> SyncResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::Shutdown { reply: reply_tx })
            .await
            .map_err(|_| SyncError::Other("Engine channel closed".to_string()))?;
        reply_rx
            .await
            .map_err(|_| SyncError::Other("Engine dropped reply channel".to_string()))?;
        Ok(())
    }
}

// ── SyncEngine ──────────────────────────────────────────────────────────

/// Per-session sync engine.
///
/// Each watched session gets its own `SyncEngine` instance running in the
/// background.  The engine manages the full sync lifecycle: file watching,
/// hashing, uploading, version creation, and checkout coordination.
pub struct SyncEngine {
    // ── Identity ────────────────────────────────────────────────────
    session_id: String,
    session_uuid: String,
    session_path: PathBuf,
    machine_id: String,

    // ── Dependencies ────────────────────────────────────────────────
    db: Arc<Database>,
    convex: Arc<ConvexClient>,
    checkout_mgr: CheckoutManager,
    wal: WriteAheadLog,
    ignore: IgnoreFilter,

    // ── Channels ────────────────────────────────────────────────────
    cmd_rx: mpsc::Receiver<EngineCommand>,
    state_tx: watch::Sender<EngineState>,

    // ── Runtime state ───────────────────────────────────────────────
    state: EngineState,
    /// Whether we currently hold the checkout.
    is_checked_out: bool,
    /// Current version number (incremented on each snapshot).
    current_version: u64,
    /// The last manifest (for diffing).
    last_manifest: Option<Manifest>,
    /// Whether an operation cancellation has been requested.
    cancelled: bool,

    // ── Configuration ───────────────────────────────────────────────
    /// Heartbeat interval (default 5 minutes).
    heartbeat_interval: Duration,
    /// Version snapshot interval (default 5 minutes).
    version_interval: Duration,
}

impl SyncEngine {
    /// Create a new engine and return a handle for controlling it.
    ///
    /// The engine does not start running until `start()` is called.
    pub fn new(
        session_id: String,
        session_uuid: String,
        session_path: PathBuf,
        machine_id: String,
        db: Arc<Database>,
        convex: Arc<ConvexClient>,
        ignore: IgnoreFilter,
    ) -> (Self, SyncEngineHandle) {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (state_tx, state_rx) = watch::channel(EngineState::Idle);

        let checkout_mgr = CheckoutManager::new(Arc::clone(&convex));
        let wal = WriteAheadLog::new(Arc::clone(&db));

        let engine = Self {
            session_id: session_id.clone(),
            session_uuid,
            session_path,
            machine_id,
            db,
            convex,
            checkout_mgr,
            wal,
            ignore,
            cmd_rx,
            state_tx,
            state: EngineState::Idle,
            is_checked_out: false,
            current_version: 0,
            last_manifest: None,
            cancelled: false,
            heartbeat_interval: Duration::from_secs(300),
            version_interval: Duration::from_secs(300),
        };

        let handle = SyncEngineHandle {
            cmd_tx,
            state_rx,
            session_id,
        };

        (engine, handle)
    }

    // ── Public entry point ──────────────────────────────────────────

    /// Start the engine's run loop.
    ///
    /// This is meant to be spawned as a tokio task:
    /// ```ignore
    /// tokio::spawn(async move { engine.start().await });
    /// ```
    ///
    /// On startup, the engine:
    /// 1. Replays any incomplete WAL entries from a previous crash.
    /// 2. Queries Convex for the latest version number.
    /// 3. Enters the main run loop, processing commands and timers.
    pub async fn start(mut self) {
        info!(
            session_id = %self.session_id,
            path = %self.session_path.display(),
            "Sync engine starting"
        );

        // ── Step 1: Crash recovery (WAL replay) ────────────────────
        if let Err(e) = self.wal.replay_all(&self.convex, &self.machine_id).await {
            error!(
                session_id = %self.session_id,
                error = %e,
                "WAL replay failed -- continuing with potentially inconsistent state"
            );
        }

        // ── Step 2: Fetch latest version number from Convex ────────
        match self.checkout_mgr.get_latest_version(&self.session_id).await {
            Ok(Some(v)) => {
                self.current_version = v;
                info!(
                    session_id = %self.session_id,
                    version = v,
                    "Fetched latest version from Convex"
                );
            }
            Ok(None) => {
                self.current_version = 0;
                debug!(
                    session_id = %self.session_id,
                    "No versions found in Convex, starting from 0"
                );
            }
            Err(e) => {
                warn!(
                    session_id = %self.session_id,
                    error = %e,
                    "Could not fetch latest version, starting from 0"
                );
            }
        }

        // ── Step 3: Main run loop ──────────────────────────────────
        self.run_loop().await;

        // ── Cleanup ────────────────────────────────────────────────
        self.set_state(EngineState::Stopped);
        info!(
            session_id = %self.session_id,
            "Sync engine stopped"
        );
    }

    // ── Main run loop ───────────────────────────────────────────────

    async fn run_loop(&mut self) {
        // Heartbeat timer (fires every 5 minutes while checked out).
        let mut heartbeat_timer = interval(self.heartbeat_interval);
        heartbeat_timer.tick().await; // discard first immediate tick

        // Version snapshot timer (fires every 5 minutes while checked out).
        let mut version_timer = interval(self.version_interval);
        version_timer.tick().await; // discard first immediate tick

        // Channel for receiving file change batches from the watcher.
        let (batch_tx, mut batch_rx) = mpsc::channel::<ChangeBatch>(64);

        // The watcher is created on-demand when we enter Watching state.
        let mut session_watcher: Option<SessionWatcher> = None;

        // Drive health check interval.
        let mut drive_check = interval(Duration::from_secs(30));
        drive_check.tick().await;

        // Accumulator for file changes between version snapshots.
        let mut pending_changes: Vec<FileEntry> = Vec::new();

        loop {
            tokio::select! {
                // ── Incoming commands ────────────────────────────────
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        EngineCommand::Checkout { reply } => {
                            let result = self.handle_checkout(
                                &batch_tx,
                                &mut session_watcher,
                            ).await;
                            let _ = reply.send(result);
                        }

                        EngineCommand::Release { note, reply } => {
                            let result = self.handle_release(
                                note.as_deref(),
                                &mut session_watcher,
                                &mut pending_changes,
                            ).await;
                            let _ = reply.send(result);
                        }

                        EngineCommand::Pull { version, reply } => {
                            let result = self.handle_pull(version).await;
                            let _ = reply.send(result);
                        }

                        EngineCommand::Cancel => {
                            self.cancelled = true;
                            info!(session_id = %self.session_id, "Operation cancel requested");
                        }

                        EngineCommand::Shutdown { reply } => {
                            info!(session_id = %self.session_id, "Shutdown requested");
                            self.set_state(EngineState::ShuttingDown);

                            // Release checkout if we hold one.
                            if self.is_checked_out {
                                if let Err(e) = self.handle_release(
                                    None,
                                    &mut session_watcher,
                                    &mut pending_changes,
                                ).await {
                                    warn!(
                                        session_id = %self.session_id,
                                        error = %e,
                                        "Failed to release checkout during shutdown"
                                    );
                                }
                            }

                            // Stop watcher if running.
                            if let Some(ref mut w) = session_watcher {
                                w.stop_watching().await;
                                session_watcher = None;
                            }

                            let _ = reply.send(());
                            return; // exit run_loop
                        }
                    }
                }

                // ── File change batch from watcher ──────────────────
                Some(batch) = batch_rx.recv(), if self.is_checked_out => {
                    self.handle_change_batch(batch, &mut pending_changes).await;
                }

                // ── Heartbeat timer ─────────────────────────────────
                _ = heartbeat_timer.tick(), if self.is_checked_out => {
                    self.handle_heartbeat().await;
                }

                // ── Version snapshot timer ──────────────────────────
                _ = version_timer.tick(), if self.is_checked_out && !pending_changes.is_empty() => {
                    self.handle_version_snapshot(&mut pending_changes).await;
                }

                // ── Drive health check ──────────────────────────────
                _ = drive_check.tick() => {
                    self.check_drive_health(&mut session_watcher).await;
                }

                // ── All channels closed ─────────────────────────────
                else => {
                    info!(session_id = %self.session_id, "All channels closed, exiting");
                    return;
                }
            }
        }
    }

    // ── Command handlers ────────────────────────────────────────────

    /// Handle a Checkout command: acquire the Convex lock, start watching.
    async fn handle_checkout(
        &mut self,
        batch_tx: &mpsc::Sender<ChangeBatch>,
        session_watcher: &mut Option<SessionWatcher>,
    ) -> SyncResult<()> {
        if self.is_checked_out {
            return Ok(()); // already checked out
        }

        // WAL: record intent to checkout.
        let wal_id = self.wal.begin_operation(
            WalOperation::Checkout,
            &self.session_id,
            &[],
            &json!({"machine_id": self.machine_id}),
        )?;

        // Convex atomic checkout.
        self.checkout_mgr
            .checkout(&self.session_id, &self.machine_id)
            .await?;

        self.is_checked_out = true;
        self.wal.complete_operation(wal_id)?;

        // Perform initial scan to establish baseline.
        info!(
            session_id = %self.session_id,
            "Performing initial scan after checkout"
        );
        self.set_state(EngineState::Pushing);

        let scan_result = {
            let session_path = self.session_path.clone();
            let session_id = self.session_id.clone();
            let ignore = self.ignore.clone();
            let db = Arc::clone(&self.db);
            tokio::task::spawn_blocking(move || {
                scanner::scan_and_diff(&session_path, &session_id, &ignore, &db)
            })
            .await
            .map_err(|e| SyncError::Other(format!("Scan task panicked: {}", e)))?
        }?;

        // Persist scan results.
        {
            let entries = scan_result.all_entries.clone();
            let diff = scan_result.diff.clone();
            let session_id = self.session_id.clone();
            let db = Arc::clone(&self.db);
            tokio::task::spawn_blocking(move || {
                scanner::persist_scan_results(&entries, &diff, &session_id, &db)
            })
            .await
            .map_err(|e| SyncError::Other(format!("Persist task panicked: {}", e)))?
            ?;
        }

        // Upload any changed files from the initial scan.
        if !scan_result.diff.is_empty() {
            self.upload_changed_files(&scan_result.all_entries, &scan_result.diff)
                .await?;
        }

        // Build and store the initial manifest.
        self.last_manifest = Some(versioner::create_manifest(
            &self.session_id,
            self.current_version,
            &scan_result.all_entries,
        ));

        // Start file watcher.
        let mut watcher_instance = SessionWatcher::new(
            self.session_path.clone(),
            self.ignore.clone(),
            batch_tx.clone(),
        );
        watcher_instance.start_watching()?;
        *session_watcher = Some(watcher_instance);

        self.set_state(EngineState::Watching);

        info!(
            session_id = %self.session_id,
            "Checkout complete, watching for changes"
        );

        Ok(())
    }

    /// Handle a Release command: final push, create release version, unlock.
    async fn handle_release(
        &mut self,
        note: Option<&str>,
        session_watcher: &mut Option<SessionWatcher>,
        pending_changes: &mut Vec<FileEntry>,
    ) -> SyncResult<()> {
        if !self.is_checked_out {
            return Err(SyncError::NotCheckedOut {
                session_id: self.session_id.clone(),
            });
        }

        // Stop watcher first so no new events come in during release.
        if let Some(ref mut w) = session_watcher {
            w.stop_watching().await;
        }
        *session_watcher = None;

        self.set_state(EngineState::Pushing);

        // WAL: record intent to release.
        let wal_id = self.wal.begin_operation(
            WalOperation::Release,
            &self.session_id,
            &[],
            &json!({"machine_id": self.machine_id, "note": note}),
        )?;

        // Final scan to catch any last-second changes.
        let scan_result = {
            let session_path = self.session_path.clone();
            let session_id = self.session_id.clone();
            let ignore = self.ignore.clone();
            let db = Arc::clone(&self.db);
            tokio::task::spawn_blocking(move || {
                scanner::scan_and_diff(&session_path, &session_id, &ignore, &db)
            })
            .await
            .map_err(|e| SyncError::Other(format!("Scan task panicked: {}", e)))?
        }?;

        // Persist and upload any remaining changes.
        if !scan_result.diff.is_empty() {
            {
                let entries = scan_result.all_entries.clone();
                let diff = scan_result.diff.clone();
                let session_id = self.session_id.clone();
                let db = Arc::clone(&self.db);
                tokio::task::spawn_blocking(move || {
                    scanner::persist_scan_results(&entries, &diff, &session_id, &db)
                })
                .await
                .map_err(|e| SyncError::Other(format!("Persist task panicked: {}", e)))?
                ?;
            }

            self.upload_changed_files(&scan_result.all_entries, &scan_result.diff)
                .await?;
        }

        // Create the release version.
        self.current_version += 1;
        let new_manifest = versioner::create_manifest(
            &self.session_id,
            self.current_version,
            &scan_result.all_entries,
        );

        let diff = if let Some(ref old_manifest) = self.last_manifest {
            versioner::diff_manifests(old_manifest, &new_manifest)
        } else {
            // First version: everything is "added".
            let empty = Manifest {
                version: 0,
                session_id: self.session_id.clone(),
                created_at: String::new(),
                files: Default::default(),
            };
            versioner::diff_manifests(&empty, &new_manifest)
        };

        let auto_summary = versioner::generate_auto_summary(&diff);
        let files_changed = diff.total_changes() as u64;
        let bytes_changed: i64 = scan_result
            .all_entries
            .iter()
            .map(|e| e.size as i64)
            .sum();

        // Upload manifest and create Convex version record.
        versioner::upload_manifest(
            &new_manifest,
            &self.session_uuid,
            &self.machine_id,
            &auto_summary,
            note,
            true, // is_release = true
            files_changed,
            bytes_changed,
            &self.convex,
        )
        .await?;

        self.last_manifest = Some(new_manifest);

        // Release the Convex lock.
        self.checkout_mgr
            .release(&self.session_id, &self.machine_id, note)
            .await?;

        self.is_checked_out = false;
        pending_changes.clear();
        self.wal.complete_operation(wal_id)?;

        self.set_state(EngineState::Idle);

        info!(
            session_id = %self.session_id,
            version = self.current_version,
            summary = %auto_summary,
            "Session released"
        );

        Ok(())
    }

    /// Handle a Pull command: download files for a specific version.
    async fn handle_pull(&mut self, version: Option<u64>) -> SyncResult<()> {
        self.set_state(EngineState::Pulling);
        self.cancelled = false;

        // Determine which version to pull.
        let target_version = match version {
            Some(v) => v,
            None => {
                // Get latest released version from Convex.
                self.checkout_mgr
                    .get_latest_version(&self.session_id)
                    .await?
                    .unwrap_or(0)
            }
        };

        if target_version == 0 {
            self.set_state(EngineState::Idle);
            return Err(SyncError::Other(
                "No versions available to pull".to_string(),
            ));
        }

        info!(
            session_id = %self.session_id,
            version = target_version,
            "Pulling version from R2"
        );

        // WAL: record intent to download.
        let wal_id = self.wal.begin_operation(
            WalOperation::Download,
            &self.session_id,
            &[],
            &json!({"version": target_version}),
        )?;

        // Download the manifest for this version.
        let manifest = versioner::download_manifest(
            &self.session_uuid,
            target_version,
            &self.machine_id,
            &self.convex,
        )
        .await?;

        // Determine which files need downloading by comparing manifest
        // against our local DB.
        let mut to_download: Vec<(String, String)> = Vec::new(); // (relative_path, blake3_hash)

        for (rel_path, entry) in &manifest.files {
            if self.cancelled {
                self.wal.complete_operation(wal_id)?;
                self.set_state(EngineState::Idle);
                return Err(SyncError::Other("Pull cancelled".to_string()));
            }

            let needs_download = match self.db.get_file(&self.session_id, rel_path)? {
                Some(record) => record.blake3_hash != entry.blake3_hash,
                None => true,
            };

            if needs_download {
                to_download.push((rel_path.clone(), entry.blake3_hash.clone()));
            }
        }

        info!(
            session_id = %self.session_id,
            total_files = manifest.files.len(),
            to_download = to_download.len(),
            "Pull diff computed"
        );

        // Request presigned download URLs in a batch.
        if !to_download.is_empty() {
            let object_keys: Vec<String> = to_download
                .iter()
                .map(|(_, hash)| format!("_objects/{}", hash))
                .collect();

            let urls = self
                .checkout_mgr
                .request_download_urls(&self.session_id, &self.machine_id, &object_keys)
                .await?;

            // Download each file.
            let http_client = reqwest::Client::new();
            let total = to_download.len() as u32;

            for (i, ((rel_path, blake3_hash), (_key, download_url))) in
                to_download.iter().zip(urls.iter()).enumerate()
            {
                if self.cancelled {
                    break;
                }

                let abs_path = self.session_path.join(rel_path);

                // Ensure parent directory exists.
                if let Some(parent) = abs_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| SyncError::FileSystem {
                        path: parent.display().to_string(),
                        source: e,
                    })?;
                }

                // Download to temp file first for atomic replacement.
                let tmp_path = format!("{}.sessionsync-tmp", abs_path.display());

                debug!(
                    path = %rel_path,
                    "Downloading file"
                );

                let response = http_client
                    .get(download_url)
                    .send()
                    .await
                    .map_err(SyncError::Http)?;

                if !response.status().is_success() {
                    warn!(
                        path = %rel_path,
                        status = %response.status(),
                        "Download failed, skipping file"
                    );
                    continue;
                }

                let bytes = response.bytes().await.map_err(SyncError::Http)?;

                // Verify hash before writing.
                let actual_hash = hasher::hash_bytes(&bytes);
                if actual_hash != *blake3_hash {
                    warn!(
                        path = %rel_path,
                        expected = %blake3_hash,
                        actual = %actual_hash,
                        "Hash mismatch after download, skipping file"
                    );
                    continue;
                }

                // Write to temp file, then atomic rename.
                std::fs::write(&tmp_path, &bytes).map_err(|e| SyncError::FileSystem {
                    path: tmp_path.clone(),
                    source: e,
                })?;

                std::fs::rename(&tmp_path, &abs_path).map_err(|e| SyncError::FileSystem {
                    path: abs_path.display().to_string(),
                    source: e,
                })?;

                // Update DB.
                let metadata =
                    std::fs::metadata(&abs_path).map_err(|e| SyncError::FileSystem {
                        path: abs_path.display().to_string(),
                        source: e,
                    })?;
                let mtime = metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;

                self.db.upsert_file(
                    &self.session_id,
                    rel_path,
                    bytes.len() as u64,
                    blake3_hash,
                    mtime,
                    SyncStatus::Synced,
                )?;

                debug!(
                    path = %rel_path,
                    progress = format!("{}/{}", i + 1, total),
                    "File downloaded and verified"
                );
            }
        }

        // Handle deletions: files in our DB but not in the pulled manifest.
        let db_files = self.db.list_files(&self.session_id)?;
        for record in &db_files {
            if !manifest.files.contains_key(&record.relative_path) {
                let abs_path = self.session_path.join(&record.relative_path);
                if abs_path.exists() {
                    debug!(path = %record.relative_path, "Removing file not in pulled version");
                    let _ = std::fs::remove_file(&abs_path);
                }
                self.db
                    .delete_file_record(&self.session_id, &record.relative_path)?;
            }
        }

        self.last_manifest = Some(manifest);
        self.current_version = target_version;
        self.wal.complete_operation(wal_id)?;
        self.set_state(EngineState::Idle);

        info!(
            session_id = %self.session_id,
            version = target_version,
            "Pull complete"
        );

        Ok(())
    }

    // ── Event handlers ──────────────────────────────────────────────

    /// Handle a batch of file changes from the watcher.
    ///
    /// For each changed file:
    /// 1. Hash the file.
    /// 2. If the hash differs from the DB, upload immediately.
    /// 3. Accumulate the change for the next version snapshot.
    async fn handle_change_batch(
        &mut self,
        batch: ChangeBatch,
        pending_changes: &mut Vec<FileEntry>,
    ) {
        if batch.events.is_empty() {
            return;
        }

        info!(
            session_id = %self.session_id,
            changes = batch.events.len(),
            "Processing file change batch"
        );

        self.set_state(EngineState::Pushing);

        for event in &batch.events {
            let rel_path = event
                .path
                .strip_prefix(&self.session_path)
                .unwrap_or(&event.path)
                .to_string_lossy()
                .to_string();

            match event.change_type {
                watcher::ChangeType::Deleted => {
                    // Mark as deleted in DB.
                    if let Err(e) =
                        self.db.delete_file_record(&self.session_id, &rel_path)
                    {
                        warn!(
                            path = %rel_path,
                            error = %e,
                            "Failed to delete file record"
                        );
                    }
                    debug!(path = %rel_path, "File deletion recorded");
                }
                watcher::ChangeType::Created | watcher::ChangeType::Modified => {
                    // Hash and upload.
                    match hasher::hash_file(&event.path) {
                        Ok(hash) => {
                            let metadata = match std::fs::metadata(&event.path) {
                                Ok(m) => m,
                                Err(e) => {
                                    warn!(
                                        path = %rel_path,
                                        error = %e,
                                        "Could not read metadata, skipping"
                                    );
                                    continue;
                                }
                            };

                            let size = metadata.len();
                            let mtime = metadata
                                .modified()
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as i64;

                            // Check if the hash actually changed from what we have in the DB.
                            let needs_upload =
                                match self.db.get_file(&self.session_id, &rel_path) {
                                    Ok(Some(record)) => record.blake3_hash != hash,
                                    _ => true,
                                };

                            // Update DB.
                            if let Err(e) = self.db.upsert_file(
                                &self.session_id,
                                &rel_path,
                                size,
                                &hash,
                                mtime,
                                SyncStatus::Pending,
                            ) {
                                warn!(
                                    path = %rel_path,
                                    error = %e,
                                    "Failed to upsert file record"
                                );
                            }

                            if needs_upload {
                                // Upload the file immediately for safety.
                                if let Err(e) = self
                                    .upload_single_file(&rel_path, &hash, &event.path)
                                    .await
                                {
                                    warn!(
                                        path = %rel_path,
                                        error = %e,
                                        "Failed to upload file"
                                    );
                                } else {
                                    // Mark as uploaded in DB.
                                    let _ = self.db.update_sync_status(
                                        &self.session_id,
                                        &rel_path,
                                        SyncStatus::Uploaded,
                                    );
                                }
                            }

                            pending_changes.push(FileEntry {
                                relative_path: rel_path,
                                absolute_path: event.path.clone(),
                                size,
                                mtime,
                                blake3_hash: hash,
                            });
                        }
                        Err(e) => {
                            warn!(
                                path = %rel_path,
                                error = %e,
                                "Failed to hash file"
                            );
                        }
                    }
                }
            }
        }

        self.set_state(EngineState::Watching);
    }

    /// Handle the heartbeat timer: send keepalive to Convex.
    async fn handle_heartbeat(&self) {
        if let Err(e) = self
            .checkout_mgr
            .send_heartbeat(&self.session_id, &self.machine_id)
            .await
        {
            warn!(
                session_id = %self.session_id,
                error = %e,
                "Heartbeat failed"
            );
        }
    }

    /// Handle the version snapshot timer: create a manifest and upload it.
    async fn handle_version_snapshot(&mut self, pending_changes: &mut Vec<FileEntry>) {
        info!(
            session_id = %self.session_id,
            pending = pending_changes.len(),
            "Creating version snapshot"
        );

        // Full scan to get the authoritative state.
        let entries = {
            let session_path = self.session_path.clone();
            let session_id = self.session_id.clone();
            let ignore = self.ignore.clone();
            let db = Arc::clone(&self.db);
            match tokio::task::spawn_blocking(move || {
                scanner::scan_session(&session_path, &session_id, &ignore, &db)
            })
            .await
            {
                Ok(Ok(entries)) => entries,
                Ok(Err(e)) => {
                    warn!(
                        session_id = %self.session_id,
                        error = %e,
                        "Scan failed during version snapshot, skipping"
                    );
                    return;
                }
                Err(e) => {
                    warn!(
                        session_id = %self.session_id,
                        error = %e,
                        "Scan task panicked during version snapshot"
                    );
                    return;
                }
            }
        };

        self.current_version += 1;
        let new_manifest = versioner::create_manifest(
            &self.session_id,
            self.current_version,
            &entries,
        );

        let diff = if let Some(ref old_manifest) = self.last_manifest {
            versioner::diff_manifests(old_manifest, &new_manifest)
        } else {
            let empty = Manifest {
                version: 0,
                session_id: self.session_id.clone(),
                created_at: String::new(),
                files: Default::default(),
            };
            versioner::diff_manifests(&empty, &new_manifest)
        };

        if diff.is_empty() {
            // No actual changes since last manifest -- don't create a version.
            self.current_version -= 1;
            debug!(
                session_id = %self.session_id,
                "No changes since last manifest, skipping version creation"
            );
            pending_changes.clear();
            return;
        }

        let auto_summary = versioner::generate_auto_summary(&diff);
        let files_changed = diff.total_changes() as u64;
        let bytes_changed: i64 = pending_changes.iter().map(|e| e.size as i64).sum();

        // WAL: record intent to create version.
        let wal_id = match self.wal.begin_operation(
            WalOperation::CreateVersion,
            &self.session_id,
            &[],
            &json!({
                "version": self.current_version,
                "auto_summary": auto_summary,
            }),
        ) {
            Ok(id) => id,
            Err(e) => {
                warn!(error = %e, "Failed to create WAL entry for version");
                self.current_version -= 1;
                return;
            }
        };

        // Upload manifest to R2 and create Convex version record.
        match versioner::upload_manifest(
            &new_manifest,
            &self.session_uuid,
            &self.machine_id,
            &auto_summary,
            None,
            false, // is_release = false (auto-snapshot)
            files_changed,
            bytes_changed,
            &self.convex,
        )
        .await
        {
            Ok(()) => {
                self.last_manifest = Some(new_manifest);
                if let Err(e) = self.wal.complete_operation(wal_id) {
                    warn!(error = %e, "Failed to complete WAL entry");
                }
                pending_changes.clear();

                info!(
                    session_id = %self.session_id,
                    version = self.current_version,
                    summary = %auto_summary,
                    "Version snapshot created"
                );
            }
            Err(e) => {
                warn!(
                    session_id = %self.session_id,
                    version = self.current_version,
                    error = %e,
                    "Failed to upload version manifest"
                );
                // Revert version number; the next timer tick will retry.
                self.current_version -= 1;
                // Still mark WAL complete to avoid replay loops -- the next
                // snapshot will create the version naturally.
                let _ = self.wal.complete_operation(wal_id);
            }
        }
    }

    /// Periodic check that the session folder is still accessible.
    async fn check_drive_health(&self, session_watcher: &mut Option<SessionWatcher>) {
        if !self.session_path.exists() {
            warn!(
                session_id = %self.session_id,
                path = %self.session_path.display(),
                "Session path no longer accessible -- drive may be disconnected"
            );
            // Stop the watcher if running (it will fail anyway).
            if let Some(ref mut w) = session_watcher {
                w.stop_watching().await;
            }
            *session_watcher = None;
        }
    }

    // ── Upload helpers ──────────────────────────────────────────────

    /// Upload a single file to R2 via a presigned URL.
    async fn upload_single_file(
        &self,
        relative_path: &str,
        blake3_hash: &str,
        abs_path: &std::path::Path,
    ) -> SyncResult<()> {
        let object_key = format!("_objects/{}", blake3_hash);

        // Request presigned upload URL.
        let urls = self
            .checkout_mgr
            .request_upload_urls(
                &self.session_id,
                &self.machine_id,
                &[object_key.clone()],
            )
            .await?;

        let (_key, upload_url) = urls
            .into_iter()
            .next()
            .ok_or_else(|| SyncError::ConvexApi {
                function: "presignedUrls:requestBatchUploadUrls".to_string(),
                message: "No URL returned".to_string(),
            })?;

        // Read file contents.
        let data = std::fs::read(abs_path).map_err(|e| SyncError::FileSystem {
            path: abs_path.display().to_string(),
            source: e,
        })?;

        // Determine content type from extension.
        let content_type = guess_content_type(relative_path);

        // Upload via PUT.
        let http_client = reqwest::Client::new();
        let response = http_client
            .put(&upload_url)
            .header("Content-Type", content_type)
            .body(data)
            .send()
            .await
            .map_err(SyncError::Http)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(SyncError::ConvexApi {
                function: "R2 upload".to_string(),
                message: format!("Upload failed with status {}: {}", status, body),
            });
        }

        debug!(
            path = %relative_path,
            r2_key = %object_key,
            "File uploaded to R2"
        );

        Ok(())
    }

    /// Upload all changed files (new + modified) from a scan diff.
    async fn upload_changed_files(
        &self,
        entries: &[FileEntry],
        diff: &ScanDiff,
    ) -> SyncResult<()> {
        // Collect files that need uploading.
        let changed_paths: std::collections::HashSet<&str> = diff
            .new_files
            .iter()
            .chain(diff.modified_files.iter())
            .map(|s| s.as_str())
            .collect();

        if changed_paths.is_empty() {
            return Ok(());
        }

        // Separate audio files and .ptx for upload ordering.
        // Audio files first (they're referenced by .ptx), .ptx last.
        let mut audio_entries = Vec::new();
        let mut ptx_entries = Vec::new();
        let mut other_entries = Vec::new();

        for entry in entries {
            if !changed_paths.contains(entry.relative_path.as_str()) {
                continue;
            }
            let lower = entry.relative_path.to_lowercase();
            if lower.ends_with(".ptx") {
                ptx_entries.push(entry);
            } else if lower.ends_with(".wav")
                || lower.ends_with(".aiff")
                || lower.ends_with(".aif")
            {
                audio_entries.push(entry);
            } else {
                other_entries.push(entry);
            }
        }

        info!(
            session_id = %self.session_id,
            audio = audio_entries.len(),
            ptx = ptx_entries.len(),
            other = other_entries.len(),
            "Uploading changed files (audio first, .ptx last)"
        );

        // WAL: record intent to upload.
        let file_paths: Vec<String> =
            changed_paths.iter().map(|s| s.to_string()).collect();
        let wal_id = self.wal.begin_operation(
            WalOperation::Upload,
            &self.session_id,
            &file_paths,
            &json!({"count": file_paths.len()}),
        )?;

        // Upload audio files, then other files, then .ptx.
        for entry in audio_entries
            .iter()
            .chain(other_entries.iter())
            .chain(ptx_entries.iter())
        {
            if let Err(e) = self
                .upload_single_file(
                    &entry.relative_path,
                    &entry.blake3_hash,
                    &entry.absolute_path,
                )
                .await
            {
                warn!(
                    path = %entry.relative_path,
                    error = %e,
                    "Failed to upload file, continuing with remaining files"
                );
                continue;
            }

            // Update DB status.
            let _ = self.db.update_sync_status(
                &self.session_id,
                &entry.relative_path,
                SyncStatus::Uploaded,
            );
        }

        self.wal.complete_operation(wal_id)?;

        Ok(())
    }

    // ── State management ────────────────────────────────────────────

    fn set_state(&mut self, new_state: EngineState) {
        if self.state != new_state {
            debug!(
                session_id = %self.session_id,
                from = %self.state,
                to = %new_state,
                "Engine state transition"
            );
            self.state = new_state;
            let _ = self.state_tx.send(new_state);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Guess a MIME content type from a file extension.
fn guess_content_type(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".wav") {
        "audio/wav"
    } else if lower.ends_with(".aiff") || lower.ends_with(".aif") {
        "audio/aiff"
    } else if lower.ends_with(".mp3") {
        "audio/mpeg"
    } else if lower.ends_with(".flac") {
        "audio/flac"
    } else if lower.ends_with(".ptx") {
        "application/octet-stream"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".mov") {
        "video/quicktime"
    } else if lower.ends_with(".mp4") {
        "video/mp4"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_state_display() {
        assert_eq!(EngineState::Idle.to_string(), "idle");
        assert_eq!(EngineState::Watching.to_string(), "watching");
        assert_eq!(EngineState::Pushing.to_string(), "pushing");
        assert_eq!(EngineState::Pulling.to_string(), "pulling");
        assert_eq!(EngineState::ShuttingDown.to_string(), "shutting_down");
        assert_eq!(EngineState::Stopped.to_string(), "stopped");
    }

    #[test]
    fn test_guess_content_type() {
        assert_eq!(guess_content_type("track.wav"), "audio/wav");
        assert_eq!(guess_content_type("track.aiff"), "audio/aiff");
        assert_eq!(guess_content_type("session.ptx"), "application/octet-stream");
        assert_eq!(guess_content_type("manifest.json"), "application/json");
        assert_eq!(guess_content_type("video.mov"), "video/quicktime");
        assert_eq!(guess_content_type("unknown.xyz"), "application/octet-stream");
    }

    #[tokio::test]
    async fn test_engine_handle_creation() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("test_engine.db");
        let db = Arc::new(Database::new(&db_path).unwrap());
        let convex = Arc::new(ConvexClient::new("https://test.convex.cloud", "token"));
        let ignore = IgnoreFilter::default();

        let (_engine, handle) = SyncEngine::new(
            "test-session".to_string(),
            "uuid-123".to_string(),
            PathBuf::from("/tmp/test-session"),
            "test-machine".to_string(),
            db,
            convex,
            ignore,
        );

        assert_eq!(handle.state(), EngineState::Idle);
        assert_eq!(handle.session_id(), "test-session");
    }
}
