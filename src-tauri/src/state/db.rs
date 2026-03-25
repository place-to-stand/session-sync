use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, info};

use crate::error::SyncError;
use crate::state::models::{
    CachedSession, FileRecord, MultipartProgress, QueuedMutation, SyncStatus, WalEntry,
    WalOperation,
};

// ── Database ─────────────────────────────────────────────────────────────────

/// Local SQLite database for caching file state, WAL entries, session state,
/// multipart upload progress, and offline Convex mutation queue.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the SQLite database at `path` and run all migrations.
    pub fn new(path: &Path) -> Result<Self, SyncError> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for concurrent reads and better crash safety.
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;

        let db = Self { conn };
        db.run_migrations()?;
        info!("SQLite database opened at {}", path.display());
        Ok(db)
    }

    // ── Migrations ───────────────────────────────────────────────────────

    fn run_migrations(&self) -> Result<(), SyncError> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS files (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id      TEXT    NOT NULL,
                relative_path   TEXT    NOT NULL,
                size_bytes      INTEGER NOT NULL,
                blake3_hash     TEXT    NOT NULL,
                mtime_ms        INTEGER NOT NULL,
                last_checked_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                sync_status     TEXT    NOT NULL DEFAULT 'pending',
                UNIQUE(session_id, relative_path)
            );

            CREATE INDEX IF NOT EXISTS idx_files_session
                ON files(session_id);

            CREATE INDEX IF NOT EXISTS idx_files_session_path
                ON files(session_id, relative_path);

            CREATE INDEX IF NOT EXISTS idx_files_sync_status
                ON files(sync_status);

            CREATE TABLE IF NOT EXISTS wal_entries (
                id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                operation_type     TEXT    NOT NULL,
                session_id         TEXT    NOT NULL,
                file_paths_json    TEXT    NOT NULL DEFAULT '[]',
                expected_state_json TEXT   NOT NULL DEFAULT '{}',
                created_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                completed_at       TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_wal_incomplete
                ON wal_entries(completed_at) WHERE completed_at IS NULL;

            CREATE TABLE IF NOT EXISTS sessions_cache (
                session_id        TEXT PRIMARY KEY,
                name              TEXT NOT NULL,
                status            TEXT NOT NULL,
                checked_out_by    TEXT,
                checked_out_at    TEXT,
                last_heartbeat_at TEXT,
                updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );

            CREATE TABLE IF NOT EXISTS multipart_progress (
                id                   INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id           TEXT    NOT NULL,
                file_path            TEXT    NOT NULL,
                upload_id            TEXT    NOT NULL,
                total_parts          INTEGER NOT NULL,
                completed_parts_json TEXT    NOT NULL DEFAULT '[]',
                created_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                UNIQUE(session_id, file_path)
            );

            CREATE TABLE IF NOT EXISTS convex_mutation_queue (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                function_name TEXT NOT NULL,
                args_json     TEXT NOT NULL,
                created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                sent_at       TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_mutation_queue_pending
                ON convex_mutation_queue(sent_at) WHERE sent_at IS NULL;
            ",
        )?;

        debug!("Database migrations completed");
        Ok(())
    }

    // ── File Operations ──────────────────────────────────────────────────

    /// Insert or update a file record. Uses SQLite UPSERT (INSERT ... ON
    /// CONFLICT ... DO UPDATE) keyed on (session_id, relative_path).
    pub fn upsert_file(
        &self,
        session_id: &str,
        relative_path: &str,
        size_bytes: u64,
        blake3_hash: &str,
        mtime_ms: i64,
        status: SyncStatus,
    ) -> Result<(), SyncError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO files (session_id, relative_path, size_bytes, blake3_hash, mtime_ms, last_checked_at, sync_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(session_id, relative_path) DO UPDATE SET
                 size_bytes      = excluded.size_bytes,
                 blake3_hash     = excluded.blake3_hash,
                 mtime_ms        = excluded.mtime_ms,
                 last_checked_at = excluded.last_checked_at,
                 sync_status     = excluded.sync_status",
            params![
                session_id,
                relative_path,
                size_bytes as i64,
                blake3_hash,
                mtime_ms,
                now,
                status.as_db_str(),
            ],
        )?;
        Ok(())
    }

    /// Retrieve a single file record by session and path.
    pub fn get_file(
        &self,
        session_id: &str,
        relative_path: &str,
    ) -> Result<Option<FileRecord>, SyncError> {
        let row = self
            .conn
            .query_row(
                "SELECT id, session_id, relative_path, size_bytes, blake3_hash,
                        mtime_ms, last_checked_at, sync_status
                 FROM files
                 WHERE session_id = ?1 AND relative_path = ?2",
                params![session_id, relative_path],
                |row| {
                    Ok(FileRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        relative_path: row.get(2)?,
                        size_bytes: row.get::<_, i64>(3)? as u64,
                        blake3_hash: row.get(4)?,
                        mtime_ms: row.get(5)?,
                        last_checked_at: parse_datetime(&row.get::<_, String>(6)?),
                        sync_status: SyncStatus::from_db_str(&row.get::<_, String>(7)?),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// List all file records for a given session.
    pub fn list_files(&self, session_id: &str) -> Result<Vec<FileRecord>, SyncError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, relative_path, size_bytes, blake3_hash,
                    mtime_ms, last_checked_at, sync_status
             FROM files
             WHERE session_id = ?1
             ORDER BY relative_path",
        )?;

        let rows = stmt.query_map(params![session_id], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                relative_path: row.get(2)?,
                size_bytes: row.get::<_, i64>(3)? as u64,
                blake3_hash: row.get(4)?,
                mtime_ms: row.get(5)?,
                last_checked_at: parse_datetime(&row.get::<_, String>(6)?),
                sync_status: SyncStatus::from_db_str(&row.get::<_, String>(7)?),
            })
        })?;

        let mut files = Vec::new();
        for row in rows {
            files.push(row?);
        }
        Ok(files)
    }

    /// Check whether a file needs re-hashing. Returns `true` if the file is
    /// not in the database OR if its size or mtime has changed since last check.
    /// This lets the scanner skip expensive BLAKE3 hashing for unchanged files.
    pub fn file_needs_rehash(
        &self,
        session_id: &str,
        relative_path: &str,
        current_size: u64,
        current_mtime_ms: i64,
    ) -> Result<bool, SyncError> {
        let result = self
            .conn
            .query_row(
                "SELECT size_bytes, mtime_ms FROM files
                 WHERE session_id = ?1 AND relative_path = ?2",
                params![session_id, relative_path],
                |row| {
                    let db_size: i64 = row.get(0)?;
                    let db_mtime: i64 = row.get(1)?;
                    Ok((db_size, db_mtime))
                },
            )
            .optional()?;

        match result {
            None => Ok(true), // file not tracked yet
            Some((db_size, db_mtime)) => {
                Ok(db_size != current_size as i64 || db_mtime != current_mtime_ms)
            }
        }
    }

    /// Delete a file record from the database.
    pub fn delete_file_record(
        &self,
        session_id: &str,
        relative_path: &str,
    ) -> Result<(), SyncError> {
        self.conn.execute(
            "DELETE FROM files WHERE session_id = ?1 AND relative_path = ?2",
            params![session_id, relative_path],
        )?;
        Ok(())
    }

    /// Update just the sync_status of an existing file record.
    pub fn update_sync_status(
        &self,
        session_id: &str,
        relative_path: &str,
        status: SyncStatus,
    ) -> Result<(), SyncError> {
        self.conn.execute(
            "UPDATE files SET sync_status = ?1, last_checked_at = ?2
             WHERE session_id = ?3 AND relative_path = ?4",
            params![status.as_db_str(), Utc::now().to_rfc3339(), session_id, relative_path],
        )?;
        Ok(())
    }

    // ── Session Cache Operations ─────────────────────────────────────────

    /// Insert or update cached session state (from Convex).
    pub fn cache_session_state(&self, session: &CachedSession) -> Result<(), SyncError> {
        self.conn.execute(
            "INSERT INTO sessions_cache (session_id, name, status, checked_out_by, checked_out_at, last_heartbeat_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(session_id) DO UPDATE SET
                 name              = excluded.name,
                 status            = excluded.status,
                 checked_out_by    = excluded.checked_out_by,
                 checked_out_at    = excluded.checked_out_at,
                 last_heartbeat_at = excluded.last_heartbeat_at,
                 updated_at        = excluded.updated_at",
            params![
                session.session_id,
                session.name,
                session.status,
                session.checked_out_by,
                session.checked_out_at.map(|dt| dt.to_rfc3339()),
                session.last_heartbeat_at.map(|dt| dt.to_rfc3339()),
                session.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Retrieve all cached sessions (for offline display).
    pub fn get_cached_sessions(&self) -> Result<Vec<CachedSession>, SyncError> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, name, status, checked_out_by, checked_out_at,
                    last_heartbeat_at, updated_at
             FROM sessions_cache
             ORDER BY name",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(CachedSession {
                session_id: row.get(0)?,
                name: row.get(1)?,
                status: row.get(2)?,
                checked_out_by: row.get(3)?,
                checked_out_at: row
                    .get::<_, Option<String>>(4)?
                    .map(|s| parse_datetime(&s)),
                last_heartbeat_at: row
                    .get::<_, Option<String>>(5)?
                    .map(|s| parse_datetime(&s)),
                updated_at: parse_datetime(&row.get::<_, String>(6)?),
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    // ── Convex Mutation Queue ────────────────────────────────────────────

    /// Queue a Convex mutation for later replay (offline mode).
    pub fn queue_mutation(
        &self,
        function_name: &str,
        args_json: &str,
    ) -> Result<i64, SyncError> {
        self.conn.execute(
            "INSERT INTO convex_mutation_queue (function_name, args_json)
             VALUES (?1, ?2)",
            params![function_name, args_json],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Get all mutations that have not yet been sent to Convex.
    pub fn get_pending_mutations(&self) -> Result<Vec<QueuedMutation>, SyncError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, function_name, args_json, created_at, sent_at
             FROM convex_mutation_queue
             WHERE sent_at IS NULL
             ORDER BY id ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(QueuedMutation {
                id: row.get(0)?,
                function_name: row.get(1)?,
                args_json: row.get(2)?,
                created_at: parse_datetime(&row.get::<_, String>(3)?),
                sent_at: None,
            })
        })?;

        let mut mutations = Vec::new();
        for row in rows {
            mutations.push(row?);
        }
        Ok(mutations)
    }

    /// Mark a queued mutation as sent successfully.
    pub fn mark_mutation_sent(&self, id: i64) -> Result<(), SyncError> {
        self.conn.execute(
            "UPDATE convex_mutation_queue SET sent_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    // ── Multipart Progress ───────────────────────────────────────────────

    /// Save or update multipart upload progress for crash recovery.
    pub fn save_multipart_progress(
        &self,
        session_id: &str,
        file_path: &str,
        upload_id: &str,
        total_parts: u32,
        completed_parts: &[u32],
    ) -> Result<(), SyncError> {
        let parts_json = serde_json::to_string(completed_parts)
            .map_err(|e| SyncError::Other(format!("Failed to serialize parts: {e}")))?;

        self.conn.execute(
            "INSERT INTO multipart_progress (session_id, file_path, upload_id, total_parts, completed_parts_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, file_path) DO UPDATE SET
                 upload_id            = excluded.upload_id,
                 total_parts          = excluded.total_parts,
                 completed_parts_json = excluded.completed_parts_json",
            params![session_id, file_path, upload_id, total_parts, parts_json],
        )?;
        Ok(())
    }

    /// Retrieve multipart progress for a specific file, if any.
    pub fn get_multipart_progress(
        &self,
        session_id: &str,
        file_path: &str,
    ) -> Result<Option<MultipartProgress>, SyncError> {
        let row = self
            .conn
            .query_row(
                "SELECT id, session_id, file_path, upload_id, total_parts,
                        completed_parts_json, created_at
                 FROM multipart_progress
                 WHERE session_id = ?1 AND file_path = ?2",
                params![session_id, file_path],
                |row| {
                    let parts_json: String = row.get(5)?;
                    let completed_parts: Vec<u32> = serde_json::from_str(&parts_json)
                        .unwrap_or_default();

                    Ok(MultipartProgress {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        file_path: row.get(2)?,
                        upload_id: row.get(3)?,
                        total_parts: row.get::<_, u32>(4)?,
                        completed_parts,
                        created_at: parse_datetime(&row.get::<_, String>(6)?),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Clear multipart progress after a successful upload (or to abandon).
    pub fn clear_multipart_progress(
        &self,
        session_id: &str,
        file_path: &str,
    ) -> Result<(), SyncError> {
        self.conn.execute(
            "DELETE FROM multipart_progress WHERE session_id = ?1 AND file_path = ?2",
            params![session_id, file_path],
        )?;
        Ok(())
    }

    // ── WAL Operations ───────────────────────────────────────────────────

    /// Create a new WAL entry before starting a sync operation.
    pub fn create_wal_entry(
        &self,
        operation: WalOperation,
        session_id: &str,
        file_paths: &[String],
        expected_state: &serde_json::Value,
    ) -> Result<i64, SyncError> {
        let paths_json = serde_json::to_string(file_paths)
            .map_err(|e| SyncError::Other(format!("Failed to serialize paths: {e}")))?;
        let state_json = serde_json::to_string(expected_state)
            .map_err(|e| SyncError::Other(format!("Failed to serialize state: {e}")))?;

        self.conn.execute(
            "INSERT INTO wal_entries (operation_type, session_id, file_paths_json, expected_state_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![operation.as_db_str(), session_id, paths_json, state_json],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Mark a WAL entry as completed (sync operation finished successfully).
    pub fn complete_wal_entry(&self, id: i64) -> Result<(), SyncError> {
        self.conn.execute(
            "UPDATE wal_entries SET completed_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Get all incomplete WAL entries (for crash recovery on startup).
    pub fn get_incomplete_wal_entries(&self) -> Result<Vec<WalEntry>, SyncError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, operation_type, session_id, file_paths_json,
                    expected_state_json, created_at, completed_at
             FROM wal_entries
             WHERE completed_at IS NULL
             ORDER BY id ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            let paths_json: String = row.get(3)?;
            let state_json: String = row.get(4)?;
            let file_paths: Vec<String> =
                serde_json::from_str(&paths_json).unwrap_or_default();
            let expected_state: serde_json::Value =
                serde_json::from_str(&state_json).unwrap_or(serde_json::Value::Null);

            Ok(WalEntry {
                id: row.get(0)?,
                operation_type: WalOperation::from_db_str(&row.get::<_, String>(1)?),
                session_id: row.get(2)?,
                file_paths,
                expected_state,
                created_at: parse_datetime(&row.get::<_, String>(5)?),
                completed_at: None,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse an RFC 3339 datetime string, falling back to `Utc::now()` on failure.
fn parse_datetime(s: &str) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::new(&path).unwrap();
        (db, dir)
    }

    #[test]
    fn test_upsert_and_get_file() {
        let (db, _dir) = test_db();

        db.upsert_file("sess1", "Audio Files/kick.wav", 1024, "abc123", 1000, SyncStatus::Pending)
            .unwrap();

        let file = db.get_file("sess1", "Audio Files/kick.wav").unwrap().unwrap();
        assert_eq!(file.session_id, "sess1");
        assert_eq!(file.relative_path, "Audio Files/kick.wav");
        assert_eq!(file.size_bytes, 1024);
        assert_eq!(file.blake3_hash, "abc123");
        assert_eq!(file.mtime_ms, 1000);
        assert_eq!(file.sync_status, SyncStatus::Pending);

        // Upsert with new hash
        db.upsert_file("sess1", "Audio Files/kick.wav", 2048, "def456", 2000, SyncStatus::Uploaded)
            .unwrap();

        let file = db.get_file("sess1", "Audio Files/kick.wav").unwrap().unwrap();
        assert_eq!(file.size_bytes, 2048);
        assert_eq!(file.blake3_hash, "def456");
        assert_eq!(file.sync_status, SyncStatus::Uploaded);
    }

    #[test]
    fn test_list_files() {
        let (db, _dir) = test_db();

        db.upsert_file("sess1", "a.wav", 100, "h1", 1000, SyncStatus::Synced).unwrap();
        db.upsert_file("sess1", "b.wav", 200, "h2", 2000, SyncStatus::Pending).unwrap();
        db.upsert_file("sess2", "c.wav", 300, "h3", 3000, SyncStatus::Pending).unwrap();

        let files = db.list_files("sess1").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, "a.wav");
        assert_eq!(files[1].relative_path, "b.wav");
    }

    #[test]
    fn test_file_needs_rehash() {
        let (db, _dir) = test_db();

        // Unknown file needs rehash
        assert!(db.file_needs_rehash("sess1", "new.wav", 100, 1000).unwrap());

        db.upsert_file("sess1", "new.wav", 100, "hash1", 1000, SyncStatus::Synced).unwrap();

        // Same size and mtime — no rehash needed
        assert!(!db.file_needs_rehash("sess1", "new.wav", 100, 1000).unwrap());

        // Changed size — needs rehash
        assert!(db.file_needs_rehash("sess1", "new.wav", 200, 1000).unwrap());

        // Changed mtime — needs rehash
        assert!(db.file_needs_rehash("sess1", "new.wav", 100, 2000).unwrap());
    }

    #[test]
    fn test_delete_file_record() {
        let (db, _dir) = test_db();

        db.upsert_file("sess1", "del.wav", 100, "h1", 1000, SyncStatus::Synced).unwrap();
        assert!(db.get_file("sess1", "del.wav").unwrap().is_some());

        db.delete_file_record("sess1", "del.wav").unwrap();
        assert!(db.get_file("sess1", "del.wav").unwrap().is_none());
    }

    #[test]
    fn test_update_sync_status() {
        let (db, _dir) = test_db();

        db.upsert_file("sess1", "x.wav", 100, "h1", 1000, SyncStatus::Pending).unwrap();
        db.update_sync_status("sess1", "x.wav", SyncStatus::Synced).unwrap();

        let file = db.get_file("sess1", "x.wav").unwrap().unwrap();
        assert_eq!(file.sync_status, SyncStatus::Synced);
    }

    #[test]
    fn test_session_cache() {
        let (db, _dir) = test_db();

        let session = CachedSession {
            session_id: "s1".to_string(),
            name: "Rivera Album".to_string(),
            status: "available".to_string(),
            checked_out_by: None,
            checked_out_at: None,
            last_heartbeat_at: None,
            updated_at: Utc::now(),
        };

        db.cache_session_state(&session).unwrap();
        let cached = db.get_cached_sessions().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].name, "Rivera Album");
    }

    #[test]
    fn test_mutation_queue() {
        let (db, _dir) = test_db();

        let id1 = db.queue_mutation("heartbeat", r#"{"machineId":"m1"}"#).unwrap();
        let id2 = db.queue_mutation("createVersion", r#"{"sessionId":"s1"}"#).unwrap();

        let pending = db.get_pending_mutations().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].function_name, "heartbeat");
        assert_eq!(pending[1].function_name, "createVersion");

        db.mark_mutation_sent(id1).unwrap();
        let pending = db.get_pending_mutations().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].function_name, "createVersion");
    }

    #[test]
    fn test_multipart_progress() {
        let (db, _dir) = test_db();

        db.save_multipart_progress("sess1", "big.wav", "upload-123", 10, &[0, 1, 2])
            .unwrap();

        let progress = db.get_multipart_progress("sess1", "big.wav").unwrap().unwrap();
        assert_eq!(progress.upload_id, "upload-123");
        assert_eq!(progress.total_parts, 10);
        assert_eq!(progress.completed_parts, vec![0, 1, 2]);

        // Update progress
        db.save_multipart_progress("sess1", "big.wav", "upload-123", 10, &[0, 1, 2, 3, 4])
            .unwrap();
        let progress = db.get_multipart_progress("sess1", "big.wav").unwrap().unwrap();
        assert_eq!(progress.completed_parts, vec![0, 1, 2, 3, 4]);

        db.clear_multipart_progress("sess1", "big.wav").unwrap();
        assert!(db.get_multipart_progress("sess1", "big.wav").unwrap().is_none());
    }

    #[test]
    fn test_wal_entries() {
        let (db, _dir) = test_db();

        let paths = vec!["a.wav".to_string(), "b.wav".to_string()];
        let state = serde_json::json!({"expected": "synced"});

        let id = db.create_wal_entry(WalOperation::Upload, "sess1", &paths, &state).unwrap();

        let incomplete = db.get_incomplete_wal_entries().unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].session_id, "sess1");
        assert_eq!(incomplete[0].file_paths, paths);

        db.complete_wal_entry(id).unwrap();
        let incomplete = db.get_incomplete_wal_entries().unwrap();
        assert!(incomplete.is_empty());
    }
}
