use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Sync Status ──────────────────────────────────────────────────────────────

/// Tracks the synchronization state of a single file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Pending,
    Uploaded,
    Downloading,
    Synced,
    Error,
}

impl SyncStatus {
    /// Convert from the string stored in SQLite.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "uploaded" => Self::Uploaded,
            "downloading" => Self::Downloading,
            "synced" => Self::Synced,
            "error" => Self::Error,
            _ => Self::Error,
        }
    }

    /// Convert to the string stored in SQLite.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Uploaded => "uploaded",
            Self::Downloading => "downloading",
            Self::Synced => "synced",
            Self::Error => "error",
        }
    }
}

// ── WAL Operations ───────────────────────────────────────────────────────────

/// Operation types tracked in the write-ahead log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalOperation {
    Upload,
    Download,
    CreateVersion,
    Checkout,
    Release,
}

impl WalOperation {
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "upload" => Self::Upload,
            "download" => Self::Download,
            "create_version" => Self::CreateVersion,
            "checkout" => Self::Checkout,
            "release" => Self::Release,
            _ => Self::Upload, // fallback
        }
    }

    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
            Self::CreateVersion => "create_version",
            Self::Checkout => "checkout",
            Self::Release => "release",
        }
    }
}

// ── File Record ──────────────────────────────────────────────────────────────

/// A tracked file in the local SQLite database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: i64,
    pub session_id: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub blake3_hash: String,
    /// Modification time in milliseconds since Unix epoch.
    pub mtime_ms: i64,
    pub last_checked_at: DateTime<Utc>,
    pub sync_status: SyncStatus,
}

// ── Cached Session ───────────────────────────────────────────────────────────

/// Local cache of session state from Convex, used for offline display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSession {
    pub session_id: String,
    pub name: String,
    pub status: String,
    pub checked_out_by: Option<String>,
    pub checked_out_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

// ── WAL Entry ────────────────────────────────────────────────────────────────

/// A write-ahead-log entry for crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    pub id: i64,
    pub operation_type: WalOperation,
    pub session_id: String,
    pub file_paths: Vec<String>,
    pub expected_state: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

// ── Multipart Progress ───────────────────────────────────────────────────────

/// Tracks in-progress multipart uploads so they can be resumed after a crash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipartProgress {
    pub id: i64,
    pub session_id: String,
    pub file_path: String,
    pub upload_id: String,
    pub total_parts: u32,
    pub completed_parts: Vec<u32>,
    pub created_at: DateTime<Utc>,
}

// ── Queued Mutation ──────────────────────────────────────────────────────────

/// A Convex mutation that was created while offline and needs to be replayed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedMutation {
    pub id: i64,
    pub function_name: String,
    pub args_json: String,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

// ── Manifest ─────────────────────────────────────────────────────────────────

/// A version manifest stored in R2 at `_versions/{uuid}/v{N}/manifest.json`.
/// Maps relative file paths to their BLAKE3 hashes and sizes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u64,
    pub session_id: String,
    pub created_at: String,
    pub files: HashMap<String, ManifestFileEntry>,
}

/// A single file entry within a manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFileEntry {
    pub blake3_hash: String,
    pub size_bytes: u64,
}

// ── Version Info ─────────────────────────────────────────────────────────────

/// Metadata about a version, as stored in Convex and cached locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version_number: u64,
    pub session_id: String,
    pub pushed_by: String,
    pub auto_summary: String,
    pub release_note: Option<String>,
    pub is_release: bool,
    pub files_changed: u64,
    pub bytes_changed: i64,
    pub created_at: DateTime<Utc>,
}

// ── Version Diff ─────────────────────────────────────────────────────

/// Diff between two version manifests: lists of added, modified, and deleted
/// file paths.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionDiff {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl VersionDiff {
    /// Returns true if there are no changes of any kind.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    /// Total number of changed paths across all categories.
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }
}

// ── Scan Diff ────────────────────────────────────────────────────────

/// Diff produced by scanning a session directory against the database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanDiff {
    /// Files present on disk but not in the database.
    pub new_files: Vec<String>,
    /// Files whose hash or size has changed since last scan.
    pub modified_files: Vec<String>,
    /// Files in the database that are no longer on disk.
    pub deleted_files: Vec<String>,
}

impl ScanDiff {
    /// Returns true if there are no changes of any kind.
    pub fn is_empty(&self) -> bool {
        self.new_files.is_empty() && self.modified_files.is_empty() && self.deleted_files.is_empty()
    }

    /// Total number of changed paths across all categories.
    pub fn total_changes(&self) -> usize {
        self.new_files.len() + self.modified_files.len() + self.deleted_files.len()
    }
}
