# 08: Implementation Phases

**Dependencies:** References features from all other docs

---

**Timeline: 60-90 days** (extended from original 33 days to account for Rust learning curve and audit findings).

## Phase 1: Scaffold + R2 Client + Pro Tools Validation (Weeks 1-2)

**Goal:** Tauri app that connects to R2 and validates Pro Tools compatibility.

**Critical first:** Set up `notify` file watcher on a real Pro Tools session folder. Record, save, bounce. Confirm no interference. If this fails, stop and redesign.

- Scaffold Tauri 2 project
- Build R2 client: single-part upload, multipart upload, streaming download
- Build BLAKE3 hasher (mmap for <1GB, streaming for >1GB)
- **Prototype the menu bar tray panel early** — verify the popover UX is snappy (no visible load time on click). If laggy, fix before proceeding.
- Build macOS Keychain integration for credential storage
- Build error types with `thiserror`
- **Test:** Upload 50MB + 500MB WAVs, download back, verify hashes. Confirm Pro Tools compatibility.

**Blockers this phase must resolve:**
- Does `notify` file watching interfere with Pro Tools?
- Is the Tauri tray panel popover snappy enough?

## Phase 2: Convex Control Plane + Credential Broker (Weeks 3-4)

**Goal:** Convex backend with sessions, machines, users, presigned URL generation. Real-time state in the menu bar.

- Set up Convex project, define schema (see `03-architecture.md`)
- Build presigned URL generation (Convex action that uses R2 credentials server-side)
- Build mutations with server-side validation: `checkoutSession`, `releaseSession`, `claimSession`, `requestPresignedUrl`
- Build machine registration + heartbeat (every 5 min)
- Build stale checkout detection (Convex scheduled function checks heartbeats)
- Build schema version compatibility check
- Integrate Convex React client into Tauri frontend
- **Test:** Two browser windows see real-time checkout state. Presigned URLs work for R2 upload/download.

## Phase 3: Local State DB + Scanner + Checkout Flow (Weeks 5-6)

**Goal:** SQLite state tracking, file scanning, checkout/release working end-to-end.

- Build SQLite schema: files table, WAL table, Convex state cache
- Build write-ahead logging (`wal.rs`)
- Build directory scanner with BLAKE3 hashing and change detection
- Build ignore patterns for Pro Tools artifacts (see `06-pro-tools.md`)
- Build checkout flow: UI button -> Convex mutation -> local engine starts watching
- Build release flow: UI button -> optional note dialog -> final push -> Convex mutation -> stop watching
- Hash caching: skip re-hashing if (path, size, mtime) unchanged
- **Test:** Check out session -> modify files -> release. Verify Convex state transitions. Verify WAL replay after simulated crash.

## Phase 4: Auto-Push Engine + File Watcher + Versioning (Weeks 7-9)

**Goal:** While checked out, file changes auto-push to R2 with version manifests.

- Build file watcher with per-file stability debounce (5s of no size change)
- Build auto-push cycle: watcher -> scan -> hash -> request presigned URLs -> upload
- Build global upload pipeline with concurrency semaphore (max 2 concurrent)
- Build version manifest creation (JSON with file paths + BLAKE3 hashes)
- Build Convex version record creation (only after all files uploaded)
- Version snapshots batched every 5 minutes (files upload immediately)
- Upload ordering: audio files first, .ptx last
- Emit progress events to frontend (current file, bytes, speed, backlog size)
- Build soft-delete: move deleted files to `_trash/` prefix
- **Test:** Check out -> create audio files + modify .ptx -> verify auto-push to R2 -> verify version manifest -> verify Convex version record.

## Phase 5: Pull + Spectator Mode + Menu Bar UI (Weeks 10-12)

**Goal:** Full pull experience with both modes. Polished menu bar panel.

- Build "Pull Released Version": download all files matching the release version's manifest
- Build "Pull Latest (Provisional)": download files from the latest auto-push snapshot
- Build pull progress UI: overall progress + ETA + per-file status + cancel + resume
- Atomic downloads: write to `.sessionsync-tmp`, verify BLAKE3 hash, rename
- Calculate expected download size from Convex metadata before starting (compare vs free disk space)
- Build full menu bar panel (see `05-ui-spec.md`): session cards, pull dropdown, release dialog, activity feed, sync backlog indicator
- Build tray icon states: idle, syncing, attention (orange badge for stale checkout or new release)
- Desktop notifications: releases, requests, stale checkout warnings, errors
- Build "Request" flow: sends notification to checkout holder
- **Test:** Full checkout -> work -> release -> pull cycle between two machines. Test spectator pull while session is checked out. Test pull progress with large sessions.

## Phase 6: Multi-Session + External Drives + Session Discovery (Weeks 13-15)

**Goal:** Watch 5+ sessions across internal and external drives.

- Build parent directory watcher: scan for `.ptx` files, auto-register in Convex
- Build manual session add: folder picker -> register in Convex
- Build drive mount/unmount detection, session status updates
- Handle disconnect: show warning, pause auto-push
- Handle reconnect: full directory scan + diff against SQLite (don't trust FSEvents)
- Independent auto-push engines per session, all sharing the global upload pipeline
- Priority queue: checked-out session gets upload priority
- **Test:** 5+ sessions across internal + external drives. Disconnect/reconnect drive. Verify upload priority.

## Phase 7: Resilience + Polish (Weeks 16-18)

**Goal:** Production-ready reliability.

- Retry with exponential backoff (1s -> 60s max), pause after 5 consecutive failures
- Auto-resume on network return (connectivity check every 30s)
- Multipart upload recovery from SQLite part tracking
- Disk space check before pulls (compare expected size vs available space, not fixed threshold)
- Launch at login (macOS login item)
- Graceful shutdown: finish current upload if <30s, release checkouts
- Post-download BLAKE3 hash verification
- Periodic full integrity scan (weekly): re-hash all files, reconcile with R2
- Convex heartbeat health display in menu bar ("Partner last synced 3 min ago" vs "Partner offline")
- Structured logging: `tracing` + file rotation (7 days)
- "Export logs" button in Settings for remote debugging
- Convex degraded mode: cached state display, queued mutations, auto-reconnect
- **Test:** Network disconnect mid-upload -> retry. App crash mid-sync -> WAL replay -> consistent state. External drive disconnect -> reconnect -> full scan. Convex down -> cached state -> reconnect -> mutation replay.
