//! Tauri event types emitted from the Rust backend to the React frontend.
//!
//! All types derive `Serialize` + `Clone` so they can be sent through
//! `tauri::Emitter::emit`. The frontend listens via `@tauri-apps/api/event`.
//!
//! Event names are defined as constants so both the emit site and
//! the TypeScript listener reference the same string.

use serde::Serialize;

// ── Event name constants ────────────────────────────────────────────────

pub const EVENT_SYNC_PROGRESS: &str = "sync-progress";
pub const EVENT_SESSION_STATE_CHANGED: &str = "session-state-changed";
pub const EVENT_PULL_PROGRESS: &str = "pull-progress";
pub const EVENT_SYNC_ERROR: &str = "sync-error";
pub const EVENT_NEW_RELEASE_AVAILABLE: &str = "new-release-available";
pub const EVENT_SESSION_REQUESTED: &str = "session-requested";
pub const EVENT_STALE_CHECKOUT_DETECTED: &str = "stale-checkout-detected";

// ── Event payloads ──────────────────────────────────────────────────────

/// Emitted periodically during an upload cycle to report per-file and
/// overall upload progress for the session that is currently checked out.
#[derive(Debug, Clone, Serialize)]
pub struct SyncProgressEvent {
    /// Convex session ID.
    pub session_id: String,
    /// Name of the file currently being uploaded.
    pub file_name: String,
    /// Bytes uploaded so far for the current file.
    pub bytes_done: u64,
    /// Total size of the current file in bytes.
    pub bytes_total: u64,
    /// Number of files already completed in this upload batch.
    pub files_done: u32,
    /// Total number of files in this upload batch.
    pub files_total: u32,
}

/// Emitted whenever a session transitions between states (e.g. from
/// `Available` to `CheckedOut`, or from `Pulling` to `Available`).
#[derive(Debug, Clone, Serialize)]
pub struct SessionStateChanged {
    /// Convex session ID.
    pub session_id: String,
    /// The new state of the session (serialized `SessionStatus` variant name).
    pub new_state: String,
}

/// Emitted periodically during a pull (download) to show the user
/// overall progress with an estimated time of arrival.
#[derive(Debug, Clone, Serialize)]
pub struct PullProgressEvent {
    /// Convex session ID.
    pub session_id: String,
    /// Total bytes downloaded so far across all files.
    pub bytes_done: u64,
    /// Total bytes to download.
    pub bytes_total: u64,
    /// Estimated seconds remaining (may be `None` early in the transfer).
    pub eta_seconds: Option<u64>,
    /// Name of the file currently being downloaded.
    pub current_file: String,
}

/// Emitted when a sync operation for a session encounters an error that
/// the user should be aware of.
#[derive(Debug, Clone, Serialize)]
pub struct SyncErrorEvent {
    /// Convex session ID (empty string for non-session-specific errors).
    pub session_id: String,
    /// Human-readable error message.
    pub message: String,
}

/// Emitted when another machine releases a new version of a session
/// that this machine has pulled. Triggers a notification badge.
#[derive(Debug, Clone, Serialize)]
pub struct NewReleaseAvailable {
    /// Convex session ID.
    pub session_id: String,
    /// The version number of the new release.
    pub version: u64,
    /// Display name of the machine that released.
    pub released_by: String,
    /// Optional release note attached to the version.
    pub note: Option<String>,
}

/// Emitted when another machine requests the session that this machine
/// currently has checked out. Used to trigger a macOS notification.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRequested {
    /// Convex session ID.
    pub session_id: String,
    /// Display name of the machine requesting.
    pub requested_by: String,
}

/// Emitted when a session checked out by a remote machine is detected as
/// stale (heartbeat expired). This machine can now claim it.
#[derive(Debug, Clone, Serialize)]
pub struct StaleCheckoutDetected {
    /// Convex session ID.
    pub session_id: String,
    /// Display name of the machine that holds the stale checkout.
    pub machine_name: String,
}
