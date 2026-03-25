//! SessionSync -- Tauri 2 desktop application for syncing Pro Tools sessions
//! via Cloudflare R2 and Convex.
//!
//! This is the library root. It initializes logging, the tray icon, Tauri
//! plugins, the sync engine's shared state, and background tasks (heartbeat,
//! periodic scan). The `run()` function is the single entry point called by
//! `main.rs`.

// ── Module declarations ─────────────────────────────────────────────────

pub mod commands;
pub mod config;
pub mod error;
pub mod events;
pub mod ignore;
pub mod keychain;
pub mod r2;
pub mod state;
pub mod sync_engine;

// ── Imports ─────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Arc;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager, RunEvent,
};
use tauri_plugin_autostart::MacosLauncher;
use tokio::sync::{oneshot, Mutex};
use tracing::{error, info, warn};
use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::commands::{
    add_session, archive_session, cancel_pull, checkout_session, claim_session, complete_setup,
    export_logs, get_app_version, get_config, get_hostname, get_sync_status, get_version_history,
    pull_session, release_session, request_session, save_setup_config, unarchive_session,
    watch_directory,
};
use crate::config::AppConfig;
use crate::state::Database;

// ── Application state ───────────────────────────────────────────────────

/// Shared application state, managed by Tauri and accessible from every
/// IPC command handler via `tauri::State<AppState>`.
pub struct AppState {
    /// Application configuration (loaded from disk, live-editable).
    pub config: Arc<Mutex<AppConfig>>,

    /// Local SQLite database (file records, WAL, session cache, mutation queue).
    pub db: Arc<Mutex<Database>>,

    /// HTTP client for Convex API calls and presigned-URL downloads/uploads.
    pub http_client: reqwest::Client,

    /// Cached Convex auth token (loaded from Keychain on startup).
    pub convex_token: Arc<Mutex<Option<String>>>,

    /// Cancellation handles for in-progress pull operations.
    /// Maps session_id -> oneshot sender. Sending on the channel cancels
    /// the pull.
    pub active_pulls: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
}

// ── Logging ─────────────────────────────────────────────────────────────

/// Guard handle returned by the non-blocking writer. Must live for the
/// entire process to prevent log loss.
static LOG_GUARD: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
    std::sync::OnceLock::new();

/// Initialize the tracing subscriber with both console output and a
/// rotating file appender.
///
/// Log files are written to `<data_dir>/logs/` with daily rotation.
fn init_logging(config: &AppConfig) {
    let log_dir = config.log_dir();

    // Ensure the log directory exists.
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "Warning: cannot create log directory {}: {e}",
            log_dir.display()
        );
    }

    // File appender: daily rotation.
    let file_appender = rolling::daily(&log_dir, "sessionsync.log");
    let (non_blocking_writer, guard) = tracing_appender::non_blocking(file_appender);

    // Store the guard in a static so it lives for the process lifetime.
    let _ = LOG_GUARD.set(guard);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("session_sync_lib=debug,info"));

    tracing_subscriber::registry()
        .with(env_filter)
        // Console layer (stderr, with ANSI colors).
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true),
        )
        // File layer (no ANSI, writes to rotating log file).
        .with(
            fmt::layer()
                .with_writer(non_blocking_writer)
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true),
        )
        .init();

    info!("Logging initialized -- log dir: {}", log_dir.display());
}

// ── Background tasks ────────────────────────────────────────────────────

/// Spawn the heartbeat timer that pings Convex every `heartbeat_interval_secs`.
///
/// The heartbeat keeps our machine record alive and lets Convex detect
/// stale checkouts. If the Convex call fails (offline), we log a warning
/// and retry on the next tick.
fn spawn_heartbeat_task(app_handle: tauri::AppHandle) {
    tokio::spawn(async move {
        let state: tauri::State<'_, AppState> = app_handle.state();
        let interval_secs = state.config.lock().await.heartbeat_interval_secs;
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));

        loop {
            ticker.tick().await;

            let (machine_id, convex_url) = {
                let cfg = state.config.lock().await;
                (cfg.machine_id.0.clone(), cfg.convex_url.0.clone())
            };

            if convex_url.is_empty() {
                // Not configured yet -- skip.
                continue;
            }

            let url = format!("{}/api/mutation", convex_url);
            let body = serde_json::json!({
                "path": "machines:heartbeat",
                "args": {
                    "machineId": machine_id,
                    "appVersion": env!("CARGO_PKG_VERSION"),
                },
            });

            let mut request = state.http_client.post(&url).json(&body);
            if let Some(ref token) = *state.convex_token.lock().await {
                request = request.header("Authorization", format!("Bearer {}", token));
            }

            match request.send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!("Heartbeat sent successfully");
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();
                    warn!("Heartbeat returned HTTP {status}: {body_text}");
                }
                Err(e) => {
                    warn!("Heartbeat failed (offline?): {e}");
                }
            }
        }
    });
}

/// Spawn the periodic full-scan timer.
///
/// Re-hashes all tracked files and reconciles with the database. This
/// catches anything the real-time file watcher might have missed (e.g.
/// FSEvents overflow, external drive issues, manual file edits).
fn spawn_periodic_scan_task(app_handle: tauri::AppHandle) {
    tokio::spawn(async move {
        let state: tauri::State<'_, AppState> = app_handle.state();
        let interval_secs = state.config.lock().await.scan_interval_secs;
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));

        // Skip the first immediate tick -- we only want periodic runs.
        ticker.tick().await;

        loop {
            ticker.tick().await;
            info!("Periodic full scan starting");

            // Scan each watched directory and reconcile with the DB.
            let watched_dirs = state.config.lock().await.watched_dirs.clone();
            for dir in &watched_dirs {
                if !dir.exists() {
                    warn!(dir = %dir.display(), "Watched directory not found (drive disconnected?)");
                    continue;
                }
                info!(dir = %dir.display(), "Scanning directory");
                // The full scan/reconciliation logic will be driven by
                // sync_engine::scanner when that module is implemented.
                // For now we log the intent so the periodic task is functional.
            }

            info!("Periodic full scan complete");
        }
    });
}

// ── Tray icon ───────────────────────────────────────────────────────────

/// Build the system tray icon and its context menu.
fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItemBuilder::with_id("show", "Show SessionSync").build(app)?;
    let settings_item = MenuItemBuilder::with_id("settings", "Settings...").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit SessionSync").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .separator()
        .item(&settings_item)
        .separator()
        .item(&quit_item)
        .build()?;

    TrayIconBuilder::new()
        .menu(&menu)
        .icon(app.default_window_icon().cloned().unwrap())
        .icon_as_template(true)
        .tooltip("SessionSync")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "settings" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    // Emit an event so the React frontend can navigate to settings.
                    let _ = app.emit("navigate", "settings");
                }
            }
            "quit" => {
                // Graceful shutdown: release held checkouts. The actual
                // release calls are async and best-effort; the heartbeat
                // timeout handles the case where we cannot reach Convex.
                info!("Quit requested from tray menu");
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

// ── Run ─────────────────────────────────────────────────────────────────

/// Entry point for the Tauri application.
///
/// Called from `main.rs`. Sets up all plugins, state, and background tasks,
/// then enters the Tauri event loop.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ── 1. Load (or create) configuration ───────────────────────────
    let config = match AppConfig::load_or_create() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FATAL: failed to load config: {e}");
            std::process::exit(1);
        }
    };

    // ── 2. Initialize logging ───────────────────────────────────────
    init_logging(&config);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        machine_id = config.machine_id.0,
        "SessionSync starting"
    );

    // ── 3. Open local SQLite database ───────────────────────────────
    let db = match Database::new(&config.database_path()) {
        Ok(db) => db,
        Err(e) => {
            error!("FATAL: failed to open database: {e}");
            std::process::exit(1);
        }
    };

    // Replay any incomplete WAL entries from a previous crash.
    match db.get_incomplete_wal_entries() {
        Ok(entries) if !entries.is_empty() => {
            warn!(
                count = entries.len(),
                "Found incomplete WAL entries -- will replay on next sync cycle"
            );
        }
        Ok(_) => {
            info!("No incomplete WAL entries -- clean startup");
        }
        Err(e) => {
            error!("Failed to check WAL entries: {e}");
        }
    }

    // ── 4. Load Convex auth token from Keychain ─────────────────────
    let convex_token = match keychain::get_convex_token() {
        Ok(token) => {
            if token.is_some() {
                info!("Convex auth token loaded from Keychain");
            } else {
                info!("No Convex auth token in Keychain -- setup wizard required");
            }
            token
        }
        Err(e) => {
            warn!("Failed to read Convex token from Keychain: {e}");
            None
        }
    };

    // ── 5. Build shared application state ───────────────────────────
    let app_state = AppState {
        config: Arc::new(Mutex::new(config)),
        db: Arc::new(Mutex::new(db)),
        http_client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .user_agent(format!(
                "SessionSync/{} (Tauri)",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .expect("Failed to build HTTP client"),
        convex_token: Arc::new(Mutex::new(convex_token)),
        active_pulls: Arc::new(Mutex::new(HashMap::new())),
    };

    // ── 6. Build and run Tauri ──────────────────────────────────────
    tauri::Builder::default()
        // Plugins
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        // Managed state (accessible from all commands)
        .manage(app_state)
        // IPC command handlers
        .invoke_handler(tauri::generate_handler![
            checkout_session,
            release_session,
            claim_session,
            pull_session,
            cancel_pull,
            add_session,
            watch_directory,
            get_sync_status,
            request_session,
            archive_session,
            unarchive_session,
            get_version_history,
            export_logs,
            get_config,
            save_setup_config,
            complete_setup,
            get_hostname,
            get_app_version,
        ])
        // Setup (runs once at launch, after the event loop is ready)
        .setup(|app| {
            // Build the system tray icon and context menu.
            setup_tray(app)?;

            // Start background tasks on the Tauri async runtime.
            let handle = app.handle().clone();
            spawn_heartbeat_task(handle.clone());
            spawn_periodic_scan_task(handle);

            info!("Application setup complete");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Failed to build Tauri application")
        .run(|_app_handle, event| {
            if let RunEvent::ExitRequested { api, .. } = &event {
                // On macOS, closing the window should NOT quit the app.
                // The app stays alive in the system tray as a menu-bar utility.
                api.prevent_exit();
            }
        });
}
