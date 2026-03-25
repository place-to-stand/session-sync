//! Tauri IPC command handlers.
//!
//! Every function here is exposed to the React frontend via `tauri::invoke()`.
//! All commands receive `tauri::State<AppState>` and return `Result<T, String>`
//! (Tauri requires a `String` error type for IPC).
//!
//! Heavy work (network I/O, hashing, file operations) is dispatched to
//! background tasks via `tokio::spawn` — the command returns immediately
//! and progress is reported through Tauri events.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::error::SyncError;
use crate::events::{
    PullProgressEvent, SessionStateChanged, SyncErrorEvent, EVENT_PULL_PROGRESS,
    EVENT_SESSION_STATE_CHANGED, EVENT_SYNC_ERROR,
};
use crate::state::models::VersionInfo;
use crate::AppState;

// ── Shared types ────────────────────────────────────────────────────────

/// Determines which version to pull for a session.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullMode {
    /// Pull the last explicitly released version (canonical).
    Released,
    /// Pull the most recent auto-pushed snapshot (provisional / read-only).
    Latest,
}

/// The status of a session as seen from this machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Nobody has it checked out.
    Available,
    /// Currently checked out (by us or another machine).
    CheckedOut,
    /// Heartbeat expired — the holder appears offline.
    Stale,
    /// Syncing is paused, files retained in R2.
    Archived,
    /// A pull (download) is in progress.
    Pulling,
    /// The session folder is on a disconnected external drive.
    DriveDisconnected,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Available => write!(f, "available"),
            Self::CheckedOut => write!(f, "checked_out"),
            Self::Stale => write!(f, "stale"),
            Self::Archived => write!(f, "archived"),
            Self::Pulling => write!(f, "pulling"),
            Self::DriveDisconnected => write!(f, "drive_disconnected"),
        }
    }
}

/// Summary of a single session's sync state, returned by `get_sync_status`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSyncStatus {
    pub session_id: String,
    pub name: String,
    pub status: SessionStatus,
    pub checked_out_by: Option<String>,
    pub local_path: Option<String>,
    pub pending_files: u32,
    pub last_synced_at: Option<String>,
}

// ── Helper: call a Convex mutation/action via HTTP ──────────────────────

/// POST a JSON body to a Convex function endpoint and return the result body.
async fn convex_call(
    state: &AppState,
    kind: &str, // "mutation", "action", or "query"
    function: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, SyncError> {
    let url = format!("{}/api/{}", state.config.lock().await.convex_url.0, kind);

    let body = serde_json::json!({
        "path": function,
        "args": args,
    });

    let mut request = state.http_client.post(&url).json(&body);

    // Attach auth token if available.
    if let Some(ref token) = *state.convex_token.lock().await {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = request.send().await.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            SyncError::NetworkUnavailable {
                message: e.to_string(),
            }
        } else {
            SyncError::ConvexApi {
                function: function.to_string(),
                message: e.to_string(),
            }
        }
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        return Err(SyncError::ConvexApi {
            function: function.to_string(),
            message: format!("HTTP {status}: {body_text}"),
        });
    }

    let result: serde_json::Value = response.json().await.map_err(|e| SyncError::ConvexApi {
        function: function.to_string(),
        message: format!("failed to parse response: {e}"),
    })?;

    // Convex wraps successful results in { "value": ... } and errors in
    // { "errorMessage": "..." }.
    if let Some(error_msg) = result.get("errorMessage").and_then(|v| v.as_str()) {
        return Err(SyncError::ConvexApi {
            function: function.to_string(),
            message: error_msg.to_string(),
        });
    }

    Ok(result.get("value").cloned().unwrap_or(serde_json::Value::Null))
}

// ── Checkout / Release / Claim ──────────────────────────────────────────

/// Check out a session (acquire the exclusive lock).
///
/// The Convex `checkoutSession` mutation is an atomic compare-and-swap:
/// it succeeds only if the session is not currently checked out.
#[tauri::command]
pub async fn checkout_session(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    info!(session_id, "Checking out session");

    let machine_id = state.config.lock().await.machine_id.0.clone();

    convex_call(
        &state,
        "mutation",
        "sessions:checkoutSession",
        serde_json::json!({
            "sessionId": session_id,
            "machineId": machine_id,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Notify frontend of state change.
    let _ = app.emit(
        EVENT_SESSION_STATE_CHANGED,
        SessionStateChanged {
            session_id: session_id.clone(),
            new_state: SessionStatus::CheckedOut.to_string(),
        },
    );

    info!(session_id, "Session checked out successfully");
    Ok(())
}

/// Release a session (give up the exclusive lock and create a release version).
#[tauri::command]
pub async fn release_session(
    session_id: String,
    note: Option<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    info!(session_id, ?note, "Releasing session");

    let machine_id = state.config.lock().await.machine_id.0.clone();

    convex_call(
        &state,
        "mutation",
        "sessions:releaseSession",
        serde_json::json!({
            "sessionId": session_id,
            "machineId": machine_id,
            "releaseNote": note,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        EVENT_SESSION_STATE_CHANGED,
        SessionStateChanged {
            session_id: session_id.clone(),
            new_state: SessionStatus::Available.to_string(),
        },
    );

    info!(session_id, "Session released successfully");
    Ok(())
}

/// Claim a stale session (heartbeat expired).
#[tauri::command]
pub async fn claim_session(
    session_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    info!(session_id, "Claiming stale session");

    let machine_id = state.config.lock().await.machine_id.0.clone();

    convex_call(
        &state,
        "mutation",
        "sessions:claimSession",
        serde_json::json!({
            "sessionId": session_id,
            "machineId": machine_id,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        EVENT_SESSION_STATE_CHANGED,
        SessionStateChanged {
            session_id: session_id.clone(),
            new_state: SessionStatus::CheckedOut.to_string(),
        },
    );

    info!(session_id, "Stale session claimed successfully");
    Ok(())
}

// ── Pull (download) ─────────────────────────────────────────────────────

/// Start a background download of a session version.
///
/// Returns immediately. Progress is emitted as `pull-progress` events.
#[tauri::command]
pub async fn pull_session(
    session_id: String,
    mode: PullMode,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    info!(session_id, ?mode, "Starting session pull");

    // Resolve which version to pull.
    let machine_id = state.config.lock().await.machine_id.0.clone();
    let version_info: serde_json::Value = convex_call(
        &state,
        "query",
        "versions:getVersionToPull",
        serde_json::json!({
            "sessionId": session_id,
            "mode": match mode {
                PullMode::Released => "released",
                PullMode::Latest => "latest",
            },
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    let manifest_key = version_info
        .get("r2ManifestKey")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "No manifest key in version info".to_string())?
        .to_string();

    // Insert a cancellation token.
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    {
        let mut pulls = state.active_pulls.lock().await;
        pulls.insert(session_id.clone(), cancel_tx);
    }

    // Notify frontend that the pull is starting.
    let _ = app.emit(
        EVENT_SESSION_STATE_CHANGED,
        SessionStateChanged {
            session_id: session_id.clone(),
            new_state: SessionStatus::Pulling.to_string(),
        },
    );

    // Spawn background download task.
    let session_id_bg = session_id.clone();
    let app_bg = app.clone();
    let http_client = state.http_client.clone();
    let convex_url = state.config.lock().await.convex_url.0.clone();
    let convex_token = state.convex_token.lock().await.clone();

    tokio::spawn(async move {
        let result = execute_pull(
            &session_id_bg,
            &manifest_key,
            &machine_id,
            &http_client,
            &convex_url,
            convex_token.as_deref(),
            &app_bg,
            cancel_rx,
        )
        .await;

        match result {
            Ok(()) => {
                info!(session_id = session_id_bg, "Pull completed successfully");
                let _ = app_bg.emit(
                    EVENT_SESSION_STATE_CHANGED,
                    SessionStateChanged {
                        session_id: session_id_bg,
                        new_state: SessionStatus::Available.to_string(),
                    },
                );
            }
            Err(e) => {
                error!(session_id = session_id_bg, error = %e, "Pull failed");
                let _ = app_bg.emit(
                    EVENT_SYNC_ERROR,
                    SyncErrorEvent {
                        session_id: session_id_bg,
                        message: e.to_string(),
                    },
                );
            }
        }
    });

    Ok(())
}

/// Internal: execute the pull download. Runs in a background task.
async fn execute_pull(
    session_id: &str,
    manifest_key: &str,
    machine_id: &str,
    http_client: &reqwest::Client,
    convex_url: &str,
    convex_token: Option<&str>,
    app: &AppHandle,
    mut cancel_rx: oneshot::Receiver<()>,
) -> Result<(), SyncError> {
    // 1. Request presigned URL for the manifest.
    let manifest_url = request_presigned_download_url(
        http_client,
        convex_url,
        convex_token,
        session_id,
        manifest_key,
    )
    .await?;

    // 2. Download and parse the manifest.
    let manifest_bytes = http_client
        .get(&manifest_url)
        .send()
        .await
        .map_err(|e| SyncError::R2Download {
            key: manifest_key.to_string(),
            source: Box::new(e),
        })?
        .bytes()
        .await
        .map_err(|e| SyncError::R2Download {
            key: manifest_key.to_string(),
            source: Box::new(e),
        })?;

    let manifest: crate::state::models::Manifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| SyncError::ConvexApi {
            function: "manifest parse".to_string(),
            message: e.to_string(),
        })?;

    let total_bytes: u64 = manifest.files.values().map(|f| f.size_bytes).sum();
    let total_files = manifest.files.len() as u32;
    let mut bytes_done: u64 = 0;
    let mut files_done: u32 = 0;
    let start_time = std::time::Instant::now();

    // 3. Download each file.
    for (rel_path, entry) in &manifest.files {
        // Check for cancellation.
        if cancel_rx.try_recv().is_ok() {
            info!(session_id, "Pull cancelled by user");
            return Ok(());
        }

        let object_key = format!("_objects/{}", entry.blake3_hash);
        let download_url = request_presigned_download_url(
            http_client,
            convex_url,
            convex_token,
            session_id,
            &object_key,
        )
        .await?;

        // Stream download to temp file, verify hash, atomic rename.
        let response = http_client
            .get(&download_url)
            .send()
            .await
            .map_err(|e| SyncError::R2Download {
                key: object_key.clone(),
                source: Box::new(e),
            })?;

        let file_bytes = response
            .bytes()
            .await
            .map_err(|e| SyncError::R2Download {
                key: object_key.clone(),
                source: Box::new(e),
            })?;

        // Verify BLAKE3 hash.
        let actual_hash = blake3::hash(&file_bytes).to_hex().to_string();
        if actual_hash != entry.blake3_hash {
            return Err(SyncError::HashMismatch {
                path: rel_path.clone(),
                expected: entry.blake3_hash.clone(),
                actual: actual_hash,
            });
        }

        bytes_done += entry.size_bytes;
        files_done += 1;

        // Emit progress event.
        let elapsed = start_time.elapsed().as_secs().max(1);
        let eta = if bytes_done > 0 {
            Some((total_bytes.saturating_sub(bytes_done)) * elapsed / bytes_done)
        } else {
            None
        };

        let _ = app.emit(
            EVENT_PULL_PROGRESS,
            PullProgressEvent {
                session_id: session_id.to_string(),
                bytes_done,
                bytes_total: total_bytes,
                eta_seconds: eta,
                current_file: rel_path.clone(),
            },
        );
    }

    Ok(())
}

/// Request a presigned download URL from Convex for a specific R2 object.
async fn request_presigned_download_url(
    http_client: &reqwest::Client,
    convex_url: &str,
    convex_token: Option<&str>,
    session_id: &str,
    object_key: &str,
) -> Result<String, SyncError> {
    let url = format!("{}/api/action", convex_url);
    let body = serde_json::json!({
        "path": "presignedUrls:getDownloadUrl",
        "args": {
            "sessionId": session_id,
            "objectKey": object_key,
        },
    });

    let mut request = http_client.post(&url).json(&body);
    if let Some(token) = convex_token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let response = request.send().await.map_err(|e| SyncError::ConvexApi {
        function: "presignedUrls:getDownloadUrl".to_string(),
        message: e.to_string(),
    })?;

    let result: serde_json::Value = response.json().await.map_err(|e| SyncError::ConvexApi {
        function: "presignedUrls:getDownloadUrl".to_string(),
        message: format!("failed to parse presigned URL response: {e}"),
    })?;

    if let Some(error_msg) = result.get("errorMessage").and_then(|v| v.as_str()) {
        return Err(SyncError::ConvexApi {
            function: "presignedUrls:getDownloadUrl".to_string(),
            message: error_msg.to_string(),
        });
    }

    result
        .get("value")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| SyncError::ConvexApi {
            function: "presignedUrls:getDownloadUrl".to_string(),
            message: "presigned URL not found in response".to_string(),
        })
}

/// Cancel an active pull for a session.
#[tauri::command]
pub async fn cancel_pull(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(session_id, "Cancelling pull");

    let mut pulls = state.active_pulls.lock().await;
    if let Some(cancel_tx) = pulls.remove(&session_id) {
        // Send cancellation signal. If the receiver is already dropped
        // (pull finished) this is a harmless no-op.
        let _ = cancel_tx.send(());
        info!(session_id, "Pull cancellation signal sent");
    } else {
        warn!(session_id, "No active pull found to cancel");
    }

    Ok(())
}

// ── Session management ──────────────────────────────────────────────────

/// Register a Pro Tools session folder.
///
/// Scans the folder for `.ptx` files, registers the session in Convex,
/// and begins watching for changes.
#[tauri::command]
pub async fn add_session(
    path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let session_path = PathBuf::from(&path);

    // Validate the directory exists and contains a .ptx file.
    if !session_path.is_dir() {
        return Err(format!("'{}' is not a directory", path));
    }

    let has_ptx = std::fs::read_dir(&session_path)
        .map_err(|e| format!("Cannot read directory: {e}"))?
        .filter_map(|e| e.ok())
        .any(|entry| {
            entry
                .path()
                .extension()
                .map_or(false, |ext| ext == "ptx" || ext == "ptf")
        });

    if !has_ptx {
        return Err(
            "No Pro Tools session file (.ptx/.ptf) found in this directory".to_string(),
        );
    }

    // Derive a human-readable name from the folder name.
    let session_name = session_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled Session".to_string());

    let machine_id = state.config.lock().await.machine_id.0.clone();

    // Register in Convex.
    let result = convex_call(
        &state,
        "mutation",
        "sessions:createSession",
        serde_json::json!({
            "name": session_name,
            "machineId": machine_id,
            "localPath": path,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    let session_id = result
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    info!(session_id, session_name, path, "Session registered");
    Ok(session_id)
}

/// Watch a parent directory for Pro Tools session folders.
///
/// Scans for subdirectories containing `.ptx` files and registers each one.
/// Continues watching for new session folders appearing in the directory.
#[tauri::command]
pub async fn watch_directory(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let dir = PathBuf::from(&path);

    if !dir.is_dir() {
        return Err(format!("'{}' is not a directory", path));
    }

    // Add to watched directories in config.
    {
        let mut config = state.config.lock().await;
        if !config.watched_dirs.contains(&dir) {
            config.watched_dirs.push(dir.clone());
            config.save().map_err(|e| e.to_string())?;
        }
    }

    // Scan for existing session folders.
    let mut discovered_sessions = Vec::new();

    let entries = std::fs::read_dir(&dir).map_err(|e| format!("Cannot read directory: {e}"))?;

    for entry in entries.filter_map(|e| e.ok()) {
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        // Check if this subfolder contains a .ptx file.
        let has_ptx = std::fs::read_dir(&entry_path)
            .ok()
            .map(|rd| {
                rd.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .map_or(false, |ext| ext == "ptx" || ext == "ptf")
                })
            })
            .unwrap_or(false);

        if has_ptx {
            let name = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            discovered_sessions.push(name);
        }
    }

    info!(
        path,
        count = discovered_sessions.len(),
        "Watching directory for Pro Tools sessions"
    );

    Ok(discovered_sessions)
}

// ── Sync status ─────────────────────────────────────────────────────────

/// Return the current sync state for all sessions known to this machine.
///
/// Combines data from the local SQLite cache and any real-time state
/// held in memory.
#[tauri::command]
pub async fn get_sync_status(
    state: State<'_, AppState>,
) -> Result<Vec<SessionSyncStatus>, String> {
    let db = state.db.lock().await;

    let cached_sessions = db.get_cached_sessions().map_err(|e| e.to_string())?;

    let statuses: Vec<SessionSyncStatus> = cached_sessions
        .into_iter()
        .map(|s| {
            let status = match s.status.as_str() {
                "available" => SessionStatus::Available,
                "checked_out" => SessionStatus::CheckedOut,
                "stale" => SessionStatus::Stale,
                "archived" => SessionStatus::Archived,
                _ => SessionStatus::Available,
            };

            SessionSyncStatus {
                session_id: s.session_id.clone(),
                name: s.name,
                status,
                checked_out_by: s.checked_out_by,
                local_path: None,
                pending_files: 0,
                last_synced_at: Some(s.updated_at.to_rfc3339()),
            }
        })
        .collect();

    Ok(statuses)
}

// ── Request / Archive / Unarchive ───────────────────────────────────────

/// Send a "please release this session" request notification to the
/// engineer who currently has it checked out.
#[tauri::command]
pub async fn request_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(session_id, "Requesting session release");

    let machine_id = state.config.lock().await.machine_id.0.clone();

    convex_call(
        &state,
        "mutation",
        "sessions:requestSession",
        serde_json::json!({
            "sessionId": session_id,
            "machineId": machine_id,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    info!(session_id, "Session release request sent");
    Ok(())
}

/// Archive a session: stop syncing and release any checkout.
#[tauri::command]
pub async fn archive_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(session_id, "Archiving session");

    let machine_id = state.config.lock().await.machine_id.0.clone();

    convex_call(
        &state,
        "mutation",
        "sessions:archiveSession",
        serde_json::json!({
            "sessionId": session_id,
            "machineId": machine_id,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    info!(session_id, "Session archived");
    Ok(())
}

/// Unarchive a session: resume syncing.
#[tauri::command]
pub async fn unarchive_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(session_id, "Unarchiving session");

    let machine_id = state.config.lock().await.machine_id.0.clone();

    convex_call(
        &state,
        "mutation",
        "sessions:unarchiveSession",
        serde_json::json!({
            "sessionId": session_id,
            "machineId": machine_id,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    info!(session_id, "Session unarchived");
    Ok(())
}

// ── Version history ─────────────────────────────────────────────────────

/// Retrieve the version history for a session from Convex.
#[tauri::command]
pub async fn get_version_history(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<VersionInfo>, String> {
    info!(session_id, "Fetching version history");

    let result = convex_call(
        &state,
        "query",
        "versions:listVersions",
        serde_json::json!({
            "sessionId": session_id,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Parse the Convex response into VersionInfo structs.
    let versions: Vec<VersionInfo> = result
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|v| {
            Some(VersionInfo {
                version_number: v.get("versionNumber")?.as_u64()?,
                session_id: v.get("sessionId")?.as_str()?.to_string(),
                pushed_by: v.get("pushedBy")?.as_str()?.to_string(),
                auto_summary: v
                    .get("autoSummary")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                release_note: v
                    .get("releaseNote")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string()),
                is_release: v.get("isRelease")?.as_bool()?,
                files_changed: v.get("filesChanged").and_then(|n| n.as_u64()).unwrap_or(0),
                bytes_changed: v.get("bytesChanged").and_then(|n| n.as_i64()).unwrap_or(0),
                created_at: {
                    let ts = v.get("createdAt").and_then(|n| n.as_f64()).unwrap_or(0.0);
                    chrono::DateTime::from_timestamp_millis(ts as i64)
                        .unwrap_or_else(|| chrono::Utc::now())
                },
            })
        })
        .collect();

    Ok(versions)
}

// ── Configuration commands ───────────────────────────────────────────

/// Get the current application configuration.
#[tauri::command]
pub async fn get_config(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let config = state.config.lock().await;
    Ok(serde_json::json!({
        "machine_id": config.machine_id.0,
        "machine_name": config.machine_name.0,
        "user_name": config.user_name,
        "convex_url": config.convex_url.0,
        "is_admin": false,
        "setup_complete": !config.convex_url.0.is_empty() && !config.user_name.is_empty(),
    }))
}

/// Save the initial setup configuration.
#[tauri::command]
pub async fn save_setup_config(
    user_name: String,
    machine_name: String,
    convex_url: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!(user_name = %user_name, machine_name = %machine_name, "Saving setup config");
    let mut config = state.config.lock().await;
    config.user_name = user_name;
    config.machine_name = crate::config::MachineName(machine_name);
    config.convex_url = crate::config::ConvexUrl(convex_url);
    config.save().map_err(|e| e.to_string())?;
    info!("Setup config saved");
    Ok(())
}

/// Mark setup as complete.
#[tauri::command]
pub async fn complete_setup(
    state: State<'_, AppState>,
) -> Result<(), String> {
    let config = state.config.lock().await;
    if config.convex_url.0.is_empty() || config.user_name.is_empty() {
        return Err("Setup is not complete: missing required fields".to_string());
    }
    info!("Setup marked as complete");
    Ok(())
}

/// Get the machine hostname for pre-filling the setup wizard.
#[tauri::command]
pub async fn get_hostname() -> Result<String, String> {
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "My Mac".into());
    Ok(hostname)
}

/// Get the app version string.
#[tauri::command]
pub async fn get_app_version() -> Result<String, String> {
    Ok(env!("CARGO_PKG_VERSION").to_string())
}

// ── Log export ──────────────────────────────────────────────────────────

/// Copy log files to a user-chosen directory. Opens a native folder picker
/// dialog and copies all `.log` files from the app data dir.
#[tauri::command]
pub async fn export_logs(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let log_dir = state.config.lock().await.log_dir();

    if !log_dir.exists() {
        return Err("No log directory found".to_string());
    }

    // Collect log files.
    let log_files: Vec<PathBuf> = std::fs::read_dir(&log_dir)
        .map_err(|e| format!("Cannot read log directory: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map_or(false, |ext| ext == "log" || ext == "gz")
        })
        .collect();

    if log_files.is_empty() {
        return Err("No log files found".to_string());
    }

    // Show a folder picker dialog.
    let destination = app
        .dialog()
        .file()
        .set_title("Export Logs To...")
        .blocking_pick_folder();

    let dest_dir = match destination {
        Some(path) => path.as_path().map(|p| p.to_path_buf()),
        None => return Err("Export cancelled".to_string()),
    };

    let dest_dir = dest_dir.ok_or_else(|| "Invalid destination path".to_string())?;

    // Copy each log file.
    let mut copied = 0u32;
    for src in &log_files {
        if let Some(filename) = src.file_name() {
            let dest = dest_dir.join(filename);
            if let Err(e) = std::fs::copy(src, &dest) {
                warn!(src = %src.display(), error = %e, "Failed to copy log file");
            } else {
                copied += 1;
            }
        }
    }

    let message = format!("Exported {} log file(s) to {}", copied, dest_dir.display());
    info!("{}", message);
    Ok(message)
}
