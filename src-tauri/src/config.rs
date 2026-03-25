use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{SyncError, SyncResult};

// ── Newtypes for clarity ────────────────────────────────────────────────

/// The URL of the Convex deployment (e.g. `https://foo-bar-123.convex.cloud`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConvexUrl(pub String);

/// A stable, locally-generated identifier for this machine (UUID v4).
/// Stored in the config file on first run and never changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineId(pub String);

/// Human-readable display name for this machine (e.g. "Austin Studio").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineName(pub String);

// ── AppConfig ───────────────────────────────────────────────────────────

/// Persisted application configuration.
///
/// Stored as JSON at `<app_data_dir>/config.json`. The config file is
/// created on first launch with sensible defaults and a freshly generated
/// `machine_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// URL of the Convex deployment.
    pub convex_url: ConvexUrl,

    /// Stable machine identifier, generated once on first launch.
    pub machine_id: MachineId,

    /// Human-readable name shown to other engineers.
    pub machine_name: MachineName,

    /// The user's display name (e.g. "Jason Desiderio").
    pub user_name: String,

    /// Parent directories being watched for Pro Tools sessions.
    pub watched_dirs: Vec<PathBuf>,

    /// Root directory for SessionSync application data (DB, logs, etc.).
    /// Defaults to the platform-standard app data directory.
    pub data_dir: PathBuf,

    /// Custom ignore patterns (gitignore-style) added by the user.
    #[serde(default)]
    pub custom_ignore_patterns: Vec<String>,

    /// Heartbeat interval in seconds. Default 300 (5 minutes).
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,

    /// Periodic full-scan interval in seconds. Default 604800 (7 days).
    #[serde(default = "default_scan_interval")]
    pub scan_interval_secs: u64,
}

fn default_heartbeat_interval() -> u64 {
    300
}

fn default_scan_interval() -> u64 {
    604_800
}

impl AppConfig {
    // ── Persistence ─────────────────────────────────────────────────

    /// Load the configuration from disk. If the config file does not exist,
    /// a new one is created with defaults and written out.
    pub fn load_or_create() -> SyncResult<Self> {
        let config_path = Self::config_file_path()?;

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path).map_err(|e| {
                SyncError::FileSystem {
                    path: config_path.display().to_string(),
                    source: e,
                }
            })?;
            let config: AppConfig =
                serde_json::from_str(&contents).map_err(|e| SyncError::ConfigInvalid {
                    message: format!("failed to parse config at {}: {e}", config_path.display()),
                })?;
            Ok(config)
        } else {
            let config = Self::create_default()?;
            config.save()?;
            Ok(config)
        }
    }

    /// Write the current configuration to disk.
    pub fn save(&self) -> SyncResult<()> {
        let config_path = Self::config_file_path()?;

        // Ensure parent directory exists.
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SyncError::FileSystem {
                path: parent.display().to_string(),
                source: e,
            })?;
        }

        let contents =
            serde_json::to_string_pretty(self).map_err(SyncError::Serialization)?;
        std::fs::write(&config_path, contents).map_err(|e| SyncError::FileSystem {
            path: config_path.display().to_string(),
            source: e,
        })?;

        Ok(())
    }

    // ── Derived paths ───────────────────────────────────────────────

    /// Path to the SQLite database.
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("sessionsync.db")
    }

    /// Directory where log files are stored.
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    // ── Private helpers ─────────────────────────────────────────────

    /// Resolve the path to `config.json` within the platform app-data dir.
    fn config_file_path() -> SyncResult<PathBuf> {
        let data_dir = Self::default_data_dir()?;
        Ok(data_dir.join("config.json"))
    }

    /// Return the platform-appropriate application data directory.
    /// On macOS: `~/Library/Application Support/com.sessionsync.app`
    fn default_data_dir() -> SyncResult<PathBuf> {
        let proj_dirs = ProjectDirs::from("com", "sessionsync", "SessionSync").ok_or_else(|| {
            SyncError::ConfigInvalid {
                message: "unable to determine application data directory".into(),
            }
        })?;
        Ok(proj_dirs.data_dir().to_path_buf())
    }

    /// Build a default config with a freshly generated machine ID.
    fn create_default() -> SyncResult<Self> {
        let data_dir = Self::default_data_dir()?;
        let machine_name = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "My Mac".into());

        Ok(AppConfig {
            convex_url: ConvexUrl(String::new()),
            machine_id: MachineId(Uuid::new_v4().to_string()),
            machine_name: MachineName(machine_name),
            user_name: String::new(),
            watched_dirs: Vec::new(),
            data_dir,
            custom_ignore_patterns: Vec::new(),
            heartbeat_interval_secs: default_heartbeat_interval(),
            scan_interval_secs: default_scan_interval(),
        })
    }
}

/// Validate that a directory path exists and is accessible.
pub fn validate_directory(path: &Path) -> SyncResult<()> {
    if !path.exists() {
        return Err(SyncError::DriveDisconnected {
            path: path.display().to_string(),
        });
    }
    if !path.is_dir() {
        return Err(SyncError::ConfigInvalid {
            message: format!("'{}' is not a directory", path.display()),
        });
    }
    Ok(())
}
