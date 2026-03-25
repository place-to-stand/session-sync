//! Write-Ahead Log for crash recovery.
//!
//! Every sync operation (upload, download, version creation, checkout, release)
//! is bracketed by WAL entries:
//!
//! 1. **Before** the operation: `begin_operation()` writes intent to SQLite.
//! 2. **Execute** R2 upload + Convex mutation.
//! 3. **After success**: `complete_operation()` marks the WAL entry done.
//!
//! On startup after a crash, `get_incomplete_entries()` returns all operations
//! that started but never completed.  `replay_entry()` inspects R2 and Convex
//! state to decide whether to finish or roll back each one.

use std::sync::Arc;

use serde_json::json;
use tracing::{error, info, warn};

use crate::error::{SyncError, SyncResult};
use crate::state::db::Database;
use crate::state::models::{WalEntry, WalOperation};

use super::checkout::ConvexClient;

/// High-level WAL interface that wraps the raw `Database` WAL methods with
/// business logic for replaying incomplete operations after a crash.
pub struct WriteAheadLog {
    db: Arc<Database>,
}

impl WriteAheadLog {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    // ── Entry lifecycle ────────────────────────────────────────────────

    /// Record intent to perform an operation. Returns the WAL entry ID.
    ///
    /// Call this **before** starting any R2 upload, Convex mutation, or
    /// multi-step sync operation so that a crash between steps can be
    /// detected and replayed on next startup.
    pub fn begin_operation(
        &self,
        op_type: WalOperation,
        session_id: &str,
        file_paths: &[String],
        expected_state: &serde_json::Value,
    ) -> SyncResult<i64> {
        let id = self
            .db
            .create_wal_entry(op_type, session_id, file_paths, expected_state)?;
        info!(
            wal_id = id,
            op = op_type.as_db_str(),
            session_id = session_id,
            files = file_paths.len(),
            "WAL: operation started"
        );
        Ok(id)
    }

    /// Mark an operation as successfully completed.
    pub fn complete_operation(&self, entry_id: i64) -> SyncResult<()> {
        self.db.complete_wal_entry(entry_id)?;
        info!(wal_id = entry_id, "WAL: operation completed");
        Ok(())
    }

    /// Retrieve all incomplete entries for crash recovery.
    pub fn get_incomplete_entries(&self) -> SyncResult<Vec<WalEntry>> {
        self.db.get_incomplete_wal_entries()
    }

    // ── Crash recovery replay ──────────────────────────────────────────

    /// Replay all incomplete WAL entries found after a crash.
    ///
    /// For each entry we inspect the current state (R2 objects, Convex session
    /// state) and decide whether to:
    /// - **Complete** the operation (e.g. the upload reached R2 but the Convex
    ///   mutation was never sent).
    /// - **Roll back** (e.g. the upload never started — just clean up the WAL).
    ///
    /// This is intentionally conservative: when in doubt we mark the entry
    /// complete and let the next sync cycle re-detect and re-upload.
    pub async fn replay_all(
        &self,
        convex: &ConvexClient,
        machine_id: &str,
    ) -> SyncResult<()> {
        let entries = self.get_incomplete_entries()?;

        if entries.is_empty() {
            info!("WAL replay: no incomplete entries found");
            return Ok(());
        }

        info!(
            count = entries.len(),
            "WAL replay: replaying incomplete entries"
        );

        for entry in &entries {
            match self.replay_entry(entry, convex, machine_id).await {
                Ok(()) => {
                    info!(
                        wal_id = entry.id,
                        op = entry.operation_type.as_db_str(),
                        "WAL replay: entry resolved"
                    );
                }
                Err(e) => {
                    error!(
                        wal_id = entry.id,
                        op = entry.operation_type.as_db_str(),
                        error = %e,
                        "WAL replay: failed to replay entry, marking complete to avoid loop"
                    );
                    // Mark complete anyway to avoid infinite replay loops.
                    // The next full scan will detect any inconsistency.
                    if let Err(mark_err) = self.complete_operation(entry.id) {
                        error!(
                            wal_id = entry.id,
                            error = %mark_err,
                            "WAL replay: could not mark failed entry as complete"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Replay a single incomplete WAL entry.
    async fn replay_entry(
        &self,
        entry: &WalEntry,
        convex: &ConvexClient,
        machine_id: &str,
    ) -> SyncResult<()> {
        match entry.operation_type {
            WalOperation::Upload => {
                self.replay_upload(entry, convex, machine_id).await?;
            }
            WalOperation::Download => {
                self.replay_download(entry).await?;
            }
            WalOperation::CreateVersion => {
                self.replay_create_version(entry, convex, machine_id).await?;
            }
            WalOperation::Checkout => {
                self.replay_checkout(entry, convex, machine_id).await?;
            }
            WalOperation::Release => {
                self.replay_release(entry, convex, machine_id).await?;
            }
        }

        // All replay paths must end by marking the entry complete.
        self.complete_operation(entry.id)?;
        Ok(())
    }

    /// Replay an incomplete upload.
    ///
    /// Strategy: check whether the files listed in the entry are already
    /// present in the SQLite file table with status "uploaded" or "synced".
    /// If so, the upload succeeded but the WAL was never marked complete.
    /// If not, reset the file status to "pending" so the next sync cycle
    /// will re-upload.
    async fn replay_upload(
        &self,
        entry: &WalEntry,
        _convex: &ConvexClient,
        _machine_id: &str,
    ) -> SyncResult<()> {
        info!(
            wal_id = entry.id,
            files = entry.file_paths.len(),
            "WAL replay: checking upload status for files"
        );

        for path in &entry.file_paths {
            // Check if the file is already recorded as uploaded/synced in our DB.
            let file_record = self.db.get_file(&entry.session_id, path)?;

            match file_record {
                Some(record) => {
                    use crate::state::models::SyncStatus;
                    match record.sync_status {
                        SyncStatus::Uploaded | SyncStatus::Synced => {
                            info!(
                                path = %path,
                                "WAL replay: file already uploaded/synced, no action needed"
                            );
                        }
                        SyncStatus::Pending | SyncStatus::Error => {
                            // File upload may not have completed — mark pending
                            // so the next push cycle re-uploads it.
                            warn!(
                                path = %path,
                                status = record.sync_status.as_db_str(),
                                "WAL replay: file not confirmed uploaded, resetting to pending"
                            );
                            self.db.update_sync_status(
                                &entry.session_id,
                                path,
                                SyncStatus::Pending,
                            )?;
                        }
                        SyncStatus::Downloading => {
                            // Shouldn't happen for an upload WAL entry, but handle gracefully.
                            warn!(
                                path = %path,
                                "WAL replay: file in downloading state during upload replay"
                            );
                        }
                    }
                }
                None => {
                    // File record not in DB at all. The scan that preceded the
                    // upload may have crashed. The next scan will re-discover it.
                    warn!(
                        path = %path,
                        "WAL replay: file not in database, will be re-discovered on next scan"
                    );
                }
            }
        }

        Ok(())
    }

    /// Replay an incomplete download.
    ///
    /// Strategy: clean up any `.sessionsync-tmp` temp files that may have
    /// been left behind. The next pull cycle will re-download as needed.
    async fn replay_download(
        &self,
        entry: &WalEntry,
    ) -> SyncResult<()> {
        info!(
            wal_id = entry.id,
            files = entry.file_paths.len(),
            "WAL replay: cleaning up incomplete downloads"
        );

        for path in &entry.file_paths {
            let tmp_path = format!("{}.sessionsync-tmp", path);
            let tmp = std::path::Path::new(&tmp_path);
            if tmp.exists() {
                info!(path = %tmp_path, "WAL replay: removing incomplete temp file");
                if let Err(e) = std::fs::remove_file(tmp) {
                    warn!(
                        path = %tmp_path,
                        error = %e,
                        "WAL replay: could not remove temp file"
                    );
                }
            }

            // Reset file status to pending so next pull cycle re-downloads.
            use crate::state::models::SyncStatus;
            if let Some(_record) = self.db.get_file(&entry.session_id, path)? {
                self.db.update_sync_status(
                    &entry.session_id,
                    path,
                    SyncStatus::Pending,
                )?;
            }
        }

        Ok(())
    }

    /// Replay an incomplete version creation.
    ///
    /// Strategy: query Convex to see if the version was actually created.
    /// If not, we don't force-create it — the next auto-push timer will
    /// create a new version naturally.
    async fn replay_create_version(
        &self,
        entry: &WalEntry,
        convex: &ConvexClient,
        _machine_id: &str,
    ) -> SyncResult<()> {
        info!(
            wal_id = entry.id,
            session_id = %entry.session_id,
            "WAL replay: checking version creation status"
        );

        // Try to query Convex for the session's latest version.
        // If Convex is unreachable, we just mark the WAL entry complete
        // and let the next sync cycle handle version creation.
        match convex
            .query(
                "versions:getLatest",
                json!({ "sessionId": entry.session_id }),
            )
            .await
        {
            Ok(response) => {
                if let Some(value) = response.get("value") {
                    if !value.is_null() {
                        info!(
                            wal_id = entry.id,
                            "WAL replay: version exists in Convex, no re-creation needed"
                        );
                    } else {
                        warn!(
                            wal_id = entry.id,
                            "WAL replay: version not found in Convex, will be created on next push cycle"
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    wal_id = entry.id,
                    error = %e,
                    "WAL replay: could not reach Convex to verify version, deferring to next cycle"
                );
            }
        }

        Ok(())
    }

    /// Replay an incomplete checkout.
    ///
    /// Strategy: query Convex to see if the checkout succeeded. If we hold
    /// the checkout, great — just mark the WAL entry complete. If not,
    /// the user will need to re-checkout.
    async fn replay_checkout(
        &self,
        entry: &WalEntry,
        convex: &ConvexClient,
        machine_id: &str,
    ) -> SyncResult<()> {
        info!(
            wal_id = entry.id,
            session_id = %entry.session_id,
            "WAL replay: verifying checkout state"
        );

        match convex
            .query(
                "sessions:get",
                json!({ "sessionId": entry.session_id }),
            )
            .await
        {
            Ok(response) => {
                if let Some(value) = response.get("value") {
                    let checked_out_by = value
                        .get("checkedOutBy")
                        .and_then(|v| v.as_str());

                    if checked_out_by == Some(machine_id) {
                        info!(
                            wal_id = entry.id,
                            "WAL replay: checkout confirmed — we hold the lock"
                        );
                    } else {
                        warn!(
                            wal_id = entry.id,
                            current_holder = ?checked_out_by,
                            "WAL replay: checkout not held by us, user must re-checkout"
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    wal_id = entry.id,
                    error = %e,
                    "WAL replay: could not verify checkout state with Convex"
                );
            }
        }

        Ok(())
    }

    /// Replay an incomplete release.
    ///
    /// Strategy: query Convex. If the session is already available (released),
    /// nothing to do. If we still hold the checkout, attempt to release again.
    async fn replay_release(
        &self,
        entry: &WalEntry,
        convex: &ConvexClient,
        machine_id: &str,
    ) -> SyncResult<()> {
        info!(
            wal_id = entry.id,
            session_id = %entry.session_id,
            "WAL replay: verifying release state"
        );

        match convex
            .query(
                "sessions:get",
                json!({ "sessionId": entry.session_id }),
            )
            .await
        {
            Ok(response) => {
                if let Some(value) = response.get("value") {
                    let status = value.get("status").and_then(|v| v.as_str());
                    let checked_out_by = value
                        .get("checkedOutBy")
                        .and_then(|v| v.as_str());

                    if status == Some("available") || checked_out_by.is_none() {
                        info!(
                            wal_id = entry.id,
                            "WAL replay: session already released"
                        );
                    } else if checked_out_by == Some(machine_id) {
                        // We still hold the checkout — try to release.
                        info!(
                            wal_id = entry.id,
                            "WAL replay: we still hold checkout, attempting release"
                        );
                        if let Err(e) = convex
                            .mutation(
                                "sessions:releaseSession",
                                json!({
                                    "sessionId": entry.session_id,
                                    "machineId": machine_id,
                                }),
                            )
                            .await
                        {
                            warn!(
                                wal_id = entry.id,
                                error = %e,
                                "WAL replay: release attempt failed, will retry on next startup"
                            );
                        }
                    } else {
                        info!(
                            wal_id = entry.id,
                            holder = ?checked_out_by,
                            "WAL replay: session checked out by someone else, our release is moot"
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    wal_id = entry.id,
                    error = %e,
                    "WAL replay: could not verify release state with Convex"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_db() -> Arc<Database> {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        // Leak the TempDir so it lives for the duration of the test.
        let db = Database::new(&path).unwrap();
        Arc::new(db)
    }

    #[test]
    fn test_begin_and_complete() {
        let db = test_db();
        let wal = WriteAheadLog::new(db);

        let paths = vec!["Audio Files/kick.wav".to_string()];
        let state = json!({"status": "uploaded"});

        let id = wal
            .begin_operation(WalOperation::Upload, "sess1", &paths, &state)
            .unwrap();

        let incomplete = wal.get_incomplete_entries().unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].id, id);
        assert_eq!(incomplete[0].session_id, "sess1");

        wal.complete_operation(id).unwrap();
        let incomplete = wal.get_incomplete_entries().unwrap();
        assert!(incomplete.is_empty());
    }

    #[test]
    fn test_multiple_entries() {
        let db = test_db();
        let wal = WriteAheadLog::new(db);

        let id1 = wal
            .begin_operation(
                WalOperation::Upload,
                "sess1",
                &["a.wav".to_string()],
                &json!({}),
            )
            .unwrap();

        let id2 = wal
            .begin_operation(
                WalOperation::CreateVersion,
                "sess1",
                &[],
                &json!({"version": 5}),
            )
            .unwrap();

        let incomplete = wal.get_incomplete_entries().unwrap();
        assert_eq!(incomplete.len(), 2);

        // Complete first, second remains
        wal.complete_operation(id1).unwrap();
        let incomplete = wal.get_incomplete_entries().unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].id, id2);
    }
}
