# SessionSync — Manual Test Plan

**Dependencies:** Reads `02-sync-model.md`, `06-pro-tools.md`, `07-sync-engine.md`

**Update this file after each coding session** — add new test cases as features are built, mark tests as passing/failing.

---

## Test Status Legend

- `[ ]` — Not yet testable (feature not built)
- `[?]` — Ready to test (feature built, not yet verified)
- `[PASS]` — Tested and passing
- `[FAIL]` — Tested and failing (see notes)
- `[SKIP]` — Skipped (not applicable for current phase)

---

## Phase 1: Scaffold + R2 + Pro Tools Validation

### PT-1: Pro Tools File Watcher Compatibility (BLOCKER)

| # | Test | Status | Notes |
|---|------|--------|-------|
| 1.1 | Set up `notify` watcher on a PT session folder, open session in Pro Tools | [ ] | |
| 1.2 | Record audio for 2 minutes while watcher is running | [ ] | |
| 1.3 | Save the session while watcher is running | [ ] | |
| 1.4 | Bounce a mix while watcher is running | [ ] | |
| 1.5 | Apply AudioSuite processing while watcher is running | [ ] | |
| 1.6 | Verify: no PT errors, no lost audio, no unexpected behavior | [ ] | |

**If any test in PT-1 fails:** STOP. File watching is a fundamental blocker. Switch to polling architecture.

### R2-1: R2 Upload/Download

| # | Test | Status | Notes |
|---|------|--------|-------|
| 1.7 | Upload a 50MB WAV to R2, download back, verify BLAKE3 hash matches | [ ] | |
| 1.8 | Upload a 500MB WAV (multipart), download back, verify hash | [ ] | |
| 1.9 | Upload a 2GB+ file (multipart, 20+ parts), verify completion | [ ] | |
| 1.10 | Abort a multipart upload midway, verify cleanup | [ ] | |
| 1.11 | R2 credentials with wrong secret → get clear error, no crash | [ ] | |

### UI-1: Tray Panel Prototype

| # | Test | Status | Notes |
|---|------|--------|-------|
| 1.12 | Click tray icon → panel appears in <200ms (no visible lag) | [ ] | |
| 1.13 | Panel displays placeholder content correctly | [ ] | |

---

## Phase 2: Convex Control Plane

### CX-1: Session Checkout

| # | Test | Status | Notes |
|---|------|--------|-------|
| 2.1 | Machine A checks out session → Convex state = "checked_out" | [ ] | |
| 2.2 | Machine B tries to check out same session → rejected with clear message | [ ] | |
| 2.3 | Machine A and B click checkout at same instant → exactly one succeeds | [ ] | |
| 2.4 | Machine A releases session → state = "available" | [ ] | |
| 2.5 | Release by non-checkout holder → rejected | [ ] | |

### CX-2: Heartbeat + Stale Detection

| # | Test | Status | Notes |
|---|------|--------|-------|
| 2.6 | Machine A checks out, sends heartbeat every 5 min → session stays "checked_out" | [ ] | |
| 2.7 | Machine A checks out, stop heartbeat, wait 30 min → session marked "stale" | [ ] | |
| 2.8 | Machine B claims stale session → succeeds, Machine B now owns checkout | [ ] | |
| 2.9 | Machine B tries to claim non-stale session → rejected | [ ] | |

### CX-3: Presigned URLs

| # | Test | Status | Notes |
|---|------|--------|-------|
| 2.10 | Request upload presigned URL for checked-out session → success | [ ] | |
| 2.11 | Request upload presigned URL for non-checked-out session → rejected | [ ] | |
| 2.12 | Request download presigned URL as non-checkout holder → success (spectator) | [ ] | |
| 2.13 | Presigned URL expires after 1 hour → upload fails with clear error | [ ] | |
| 2.14 | Request multipart presigned URLs (batch) → all URLs returned in one call | [ ] | |

### CX-4: Real-Time Subscriptions

| # | Test | Status | Notes |
|---|------|--------|-------|
| 2.15 | Machine A checks out → Machine B sees state change within 2 seconds | [ ] | |
| 2.16 | Machine A releases → Machine B sees "Updates available" within 2 seconds | [ ] | |

---

## Phase 3: Local State DB + Checkout Flow

### DB-1: File Scanner

| # | Test | Status | Notes |
|---|------|--------|-------|
| 3.1 | Scan a session folder → all audio + .ptx files found, WaveCache.wfm ignored | [ ] | |
| 3.2 | Modify a file → re-scan detects change (hash differs) | [ ] | |
| 3.3 | Add a new file → re-scan detects new file | [ ] | |
| 3.4 | Delete a file → re-scan detects deletion | [ ] | |
| 3.5 | File unchanged (same path, size, mtime) → hash NOT recomputed (cached) | [ ] | |

### DB-2: WAL Crash Recovery

| # | Test | Status | Notes |
|---|------|--------|-------|
| 3.6 | Start a sync operation, kill the process mid-upload → restart → WAL replays | [ ] | |
| 3.7 | WAL replay reconciles R2 state (file was uploaded) → SQLite updated | [ ] | |
| 3.8 | WAL replay finds upload did NOT complete → re-uploads | [ ] | |

### DB-3: End-to-End Checkout/Release

| # | Test | Status | Notes |
|---|------|--------|-------|
| 3.9 | Click "Check Out" in UI → Convex mutation → file watcher starts | [ ] | |
| 3.10 | Click "Release" → optional note dialog shown → final push → lock released | [ ] | |
| 3.11 | Release with empty note → version saved with auto-summary only | [ ] | |

---

## Phase 4: Auto-Push + Versioning

### AP-1: Auto-Push

| # | Test | Status | Notes |
|---|------|--------|-------|
| 4.1 | Check out → create new WAV file → file appears in R2 within 60s | [ ] | |
| 4.2 | Check out → save .ptx → .ptx uploaded after 5s debounce | [ ] | |
| 4.3 | Check out → record audio (file growing) → upload deferred until stable | [ ] | |
| 4.4 | Upload ordering: audio files upload before .ptx in same batch | [ ] | |

### AP-2: Version Snapshots

| # | Test | Status | Notes |
|---|------|--------|-------|
| 4.5 | After 5 minutes of changes → version manifest.json created in R2 | [ ] | |
| 4.6 | Version record created in Convex only AFTER all files confirmed uploaded | [ ] | |
| 4.7 | Auto-summary generated correctly ("3 new audio files, .ptx modified") | [ ] | |

### AP-3: Soft Delete

| # | Test | Status | Notes |
|---|------|--------|-------|
| 4.8 | Delete a file from session folder → file moves to `_trash/` in R2 | [ ] | |
| 4.9 | Deleted file recoverable through R2 console | [ ] | |

---

## Phase 5: Pull + Spectator Mode + Menu Bar UI

### PL-1: Pull Released Version

| # | Test | Status | Notes |
|---|------|--------|-------|
| 5.1 | Machine B pulls released session → all files download correctly | [ ] | |
| 5.2 | All downloaded files pass BLAKE3 hash verification | [ ] | |
| 5.3 | Pull progress UI shows accurate progress + ETA | [ ] | |
| 5.4 | Cancel mid-pull → downloaded files kept, can resume later | [ ] | |
| 5.5 | Pull when insufficient disk space → warning before starting | [ ] | |

### PL-2: Spectator Pull

| # | Test | Status | Notes |
|---|------|--------|-------|
| 5.6 | Machine B spectator-pulls while Machine A has checkout → latest state downloads | [ ] | |
| 5.7 | Spectator pull clearly labeled as "provisional" in UI | [ ] | |
| 5.8 | Spectator pull does NOT acquire checkout | [ ] | |

### PL-3: Menu Bar Panel

| # | Test | Status | Notes |
|---|------|--------|-------|
| 5.9 | All session states display correctly (see `02-sync-model.md` states table) | [ ] | |
| 5.10 | Pull dropdown shows correct options per state | [ ] | |
| 5.11 | Release dialog shows auto-summary + optional note field | [ ] | |
| 5.12 | Activity feed shows recent events | [ ] | |
| 5.13 | Tray icon changes state: idle → syncing → attention | [ ] | |
| 5.14 | Desktop notification on new release from partner | [ ] | |

### PL-4: Request Flow

| # | Test | Status | Notes |
|---|------|--------|-------|
| 5.15 | Machine B clicks "Request" → Machine A gets macOS notification | [ ] | |
| 5.16 | Machine A is offline → request stored, visible in activity feed on return | [ ] | |

---

## Phase 6: Multi-Session + External Drives

### MS-1: Multi-Session

| # | Test | Status | Notes |
|---|------|--------|-------|
| 6.1 | 5+ sessions visible in menu bar simultaneously | [ ] | |
| 6.2 | Check out session A, session B checked out by partner → both states correct | [ ] | |
| 6.3 | Upload priority: checked-out session uploads before others | [ ] | |

### MS-2: Session Discovery

| # | Test | Status | Notes |
|---|------|--------|-------|
| 6.4 | Watch parent dir → new session folder with .ptx auto-discovered | [ ] | |
| 6.5 | Manually add session folder → registered in Convex | [ ] | |

### MS-3: External Drives

| # | Test | Status | Notes |
|---|------|--------|-------|
| 6.6 | Session on external drive → syncs normally | [ ] | |
| 6.7 | Disconnect external drive → session shows "Drive disconnected" (no error spam) | [ ] | |
| 6.8 | Reconnect drive → full scan → changes detected and synced | [ ] | |

---

## Phase 7: Resilience

### RS-1: Network Failures

| # | Test | Status | Notes |
|---|------|--------|-------|
| 7.1 | Disconnect wifi mid-upload → retries with exponential backoff | [ ] | |
| 7.2 | 5 consecutive failures → auto-push pauses, error shown in UI | [ ] | |
| 7.3 | Reconnect wifi → sync resumes automatically | [ ] | |
| 7.4 | Multipart upload interrupted → resumes from last successful part | [ ] | |

### RS-2: Convex Outage

| # | Test | Status | Notes |
|---|------|--------|-------|
| 7.5 | Convex unreachable → cached state displayed with "offline" warning | [ ] | |
| 7.6 | New checkout blocked while Convex offline | [ ] | |
| 7.7 | Convex returns → queued mutations replayed | [ ] | |

### RS-3: App Lifecycle

| # | Test | Status | Notes |
|---|------|--------|-------|
| 7.8 | Quit app → held checkouts released | [ ] | |
| 7.9 | Crash mid-sync → relaunch → WAL replay → consistent state | [ ] | |
| 7.10 | Launch at login → app starts in tray | [ ] | |

### RS-4: Data Integrity

| # | Test | Status | Notes |
|---|------|--------|-------|
| 7.11 | Corrupt a downloaded file → weekly integrity scan detects and re-downloads | [ ] | |
| 7.12 | Every download verified with BLAKE3 hash → mismatch rejected | [ ] | |

---

## End-to-End Scenarios

### E2E-1: Full Workflow

| # | Test | Status | Notes |
|---|------|--------|-------|
| E1 | Machine A: pull session → check out → record audio → save → auto-push → release with note | [ ] | |
| E2 | Machine B: sees release notification → pull released version → check out → work → release | [ ] | |
| E3 | Machine A: spectator pull while Machine B has checkout → latest provisional state | [ ] | |

### E2E-2: Conflict Prevention

| # | Test | Status | Notes |
|---|------|--------|-------|
| E4 | Two machines try checkout at same instant → one wins, one gets clear rejection | [ ] | |
| E5 | Machine crashes with checkout → heartbeat expires → other machine claims | [ ] | |

---

*Update status markers after each coding session. Add new test cases as features are built.*
