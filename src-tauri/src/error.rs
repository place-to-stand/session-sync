use thiserror::Error;

/// Unified error type for the SessionSync application.
///
/// Every fallible operation in the sync engine, R2 client, Convex API layer,
/// and IPC command handlers propagates errors through this type. The `Display`
/// impl (derived by `thiserror`) produces user-facing messages that are safe
/// to surface in the frontend UI.
#[derive(Error, Debug)]
pub enum SyncError {
    // ── R2 / S3 storage errors ──────────────────────────────────────────

    #[error("R2 upload failed for '{key}': {source}")]
    R2Upload {
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("R2 download failed for '{key}': {source}")]
    R2Download {
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Presigned URL for '{key}' has expired; requesting a new one")]
    PresignedUrlExpired { key: String },

    // ── Convex API errors ───────────────────────────────────────────────

    #[error("Convex API error calling '{function}': {message}")]
    ConvexApi { function: String, message: String },

    // ── Local database errors ───────────────────────────────────────────

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    // ── Filesystem errors ───────────────────────────────────────────────

    #[error("Filesystem error at '{path}': {source}")]
    FileSystem {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("BLAKE3 hash mismatch for '{path}': expected {expected}, got {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error(
        "External drive disconnected: session folder '{path}' is not accessible"
    )]
    DriveDisconnected { path: String },

    // ── Checkout / session state errors ──────────────────────────────────

    #[error("Checkout conflict: session '{session_id}' is already checked out by '{held_by}'")]
    CheckoutConflict {
        session_id: String,
        held_by: String,
    },

    #[error("Session '{session_id}' is not checked out by this machine")]
    NotCheckedOut { session_id: String },

    // ── Network errors ──────────────────────────────────────────────────

    #[error("Network unavailable: {message}")]
    NetworkUnavailable { message: String },

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    // ── Configuration errors ────────────────────────────────────────────

    #[error("Configuration invalid: {message}")]
    ConfigInvalid { message: String },

    // ── Keychain errors ─────────────────────────────────────────────────

    #[error("Keychain error: {message}")]
    KeychainError { message: String },

    // ── Serialization errors ────────────────────────────────────────────

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    // ── Generic / catch-all ─────────────────────────────────────────────

    #[error("{0}")]
    Other(String),
}

// ── Conversion: SyncError -> String (for Tauri IPC command Results) ──────

impl From<SyncError> for String {
    fn from(err: SyncError) -> Self {
        err.to_string()
    }
}

// ── Convenience conversions ─────────────────────────────────────────────

impl From<std::io::Error> for SyncError {
    fn from(err: std::io::Error) -> Self {
        SyncError::FileSystem {
            path: String::new(),
            source: err,
        }
    }
}

/// Convenience alias used throughout the crate.
pub type SyncResult<T> = Result<T, SyncError>;
