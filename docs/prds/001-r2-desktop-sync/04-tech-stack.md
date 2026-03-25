# 04: Technology Stack & Project Structure

**Dependencies:** Reads `03-architecture.md` (references Convex schema)

---

## Technology Stack

| Layer | Choice | Why |
|-------|--------|-----|
| Desktop framework | **Tauri 2** (stable) | 30MB idle RAM vs Electron's 300MB. Engineers need CPU/RAM for Pro Tools. |
| Backend language | **Rust** | File I/O, hashing, and uploads are CPU-bound. Zero GC pauses. |
| Control plane | **Convex** | Real-time subscriptions, credential broker, user/machine identity. |
| File storage | **Cloudflare R2** | $0 egress, S3-compatible, $0.015/GB/month. |
| File watching | `notify` v7 + `notify-debouncer-full` | Cross-platform, rename detection, configurable debounce. |
| R2 API | `aws-sdk-s3` (official AWS Rust SDK) | Mature SDK with multipart upload support. Uses presigned URLs from Convex. |
| Local state DB | `rusqlite` (bundled SQLite) | ACID guarantees, crash recovery via write-ahead logging. |
| File hashing | `blake3` with mmap (files <1GB) / streaming (files >1GB) | ~1 GB/s hashing. Streaming for large files avoids memory pressure when Pro Tools is running. |
| Async runtime | `tokio` | Already used by Tauri internally. |
| Frontend | React + Tailwind + Convex React client | Menu bar panel + settings UI. Real-time subscriptions via Convex hooks. |
| Credentials | macOS Keychain | Convex auth token stored in Keychain, not plaintext JSON. |

## Project Structure

```
session-sync/
├── src/                              # React frontend
│   ├── App.tsx
│   ├── components/
│   │   ├── SetupWizard.tsx           # First-run experience
│   │   ├── MenuBarPanel.tsx          # Main menu bar dropdown UI
│   │   ├── SessionCard.tsx           # Individual session status + actions
│   │   ├── ReleaseDialog.tsx         # Auto-summary + optional note on release
│   │   ├── PullProgress.tsx          # Download progress with ETA + per-file status
│   │   ├── VersionHistory.tsx        # Version list with rollback
│   │   ├── ActivityFeed.tsx          # Recent push/pull/checkout events
│   │   ├── SyncBacklog.tsx           # "Keeping up" indicator during active recording
│   │   └── Settings.tsx              # Config, ignore patterns, log viewer, export logs
│   ├── hooks/
│   │   ├── useTauriEvents.ts         # Listen to Rust-emitted events
│   │   └── useConvex.ts              # Real-time Convex subscriptions
│   └── lib/
│       ├── commands.ts               # Typed wrappers for Tauri invoke()
│       └── convex/                   # Convex client config
├── convex/                           # Convex backend
│   ├── schema.ts                     # Table definitions
│   ├── sessions.ts                   # Checkout/release/claim mutations
│   ├── versions.ts                   # Version history queries
│   ├── machines.ts                   # Registration, heartbeat, stale detection
│   ├── presignedUrls.ts             # R2 presigned URL generation
│   ├── activity.ts                   # Activity feed queries
│   └── config.ts                     # Schema version, min client version
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs                   # Entry point
│       ├── lib.rs                    # App setup, tray icon, plugin registration
│       ├── commands.rs               # Tauri IPC command handlers
│       ├── config.rs                 # Machine ID, Convex URL, watched folders
│       ├── ignore.rs                 # Pro Tools ignore patterns
│       ├── error.rs                  # Unified error types (thiserror)
│       ├── events.rs                 # Event types emitted to frontend
│       ├── keychain.rs              # macOS Keychain read/write for Convex auth token
│       ├── sync_engine/
│       │   ├── mod.rs                # Orchestrator: state machine, sync loop, WAL replay
│       │   ├── watcher.rs            # File system watcher + per-file stability debounce
│       │   ├── hasher.rs             # BLAKE3 hashing (mmap <1GB, streaming >1GB)
│       │   ├── scanner.rs            # Directory walking, change detection vs SQLite
│       │   ├── versioner.rs          # Version snapshot + manifest creation
│       │   ├── checkout.rs           # Checkout/release state coordination with Convex
│       │   └── wal.rs               # Write-ahead log for crash recovery
│       ├── r2/
│       │   ├── mod.rs                # S3 client using presigned URLs from Convex
│       │   ├── upload.rs             # Single + multipart upload with global concurrency limit
│       │   ├── download.rs           # Streaming download + atomic rename + hash verify
│       │   └── trash.rs              # Soft-delete: move to _trash/ prefix
│       └── state/
│           ├── db.rs                 # SQLite schema, migrations, queries, Convex cache
│           └── models.rs             # FileRecord, SyncState, SessionConfig, WALEntry
└── docs/
    └── prds/
        └── 001-r2-desktop-sync/      # PRD documents
```

## Key Dependencies

### Rust (src-tauri/Cargo.toml)

```toml
tauri = { version = "2", features = ["tray-icon", "image-png"] }
tauri-plugin-dialog = "2"
tauri-plugin-notification = "2"
tauri-plugin-autostart = "2"
tokio = { version = "1", features = ["full"] }
aws-sdk-s3 = "1"
aws-config = { version = "1", features = ["behavior-version-latest"] }
notify = "7"
notify-debouncer-full = "0.4"
rusqlite = { version = "0.32", features = ["bundled"] }
blake3 = { version = "1", features = ["mmap"] }
thiserror = "2"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
tracing = "0.1"
tracing-subscriber = "0.3"
tracing-appender = "0.2"
directories = "5"
glob = "0.3"
security-framework = "2"          # macOS Keychain access
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }  # Convex HTTP API client
```

### Frontend (package.json)

```json
{
  "convex": "^1.x",
  "react": "^19",
  "tailwindcss": "^4",
  "@tauri-apps/api": "^2",
  "@tauri-apps/plugin-dialog": "^2",
  "@tauri-apps/plugin-notification": "^2"
}
```

**Note:** `tauri-plugin-store` removed — credentials now in macOS Keychain (Rust-side) and Convex (server-side).
