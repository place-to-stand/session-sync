# 07: Sync Engine Internals

**Dependencies:** Reads `03-architecture.md` (R2, presigned URLs, Convex communication)

---

## Crash Recovery: Write-Ahead Logging

To handle crashes between R2 upload and Convex mutation (or between Convex mutation and SQLite update):

1. **Before any sync operation:** write intent to SQLite WAL table (operation type, file paths, expected state)
2. **Execute R2 upload + Convex mutation**
3. **On success:** mark WAL entry as complete, update SQLite file state
4. **On crash:** next startup replays incomplete WAL entries — checks R2 state, reconciles with Convex, updates SQLite

This ensures no sync operation is half-done after a crash.

## Upload Pipeline

### Global Concurrency Limit

All sessions share a single upload pipeline with a global semaphore:

- **Max 2 concurrent uploads** across all sessions (prevents saturating disk I/O and network)
- **Priority queue:** currently-checked-out session gets priority over background auto-push for other sessions
- **Upload ordering within a batch:** audio files first (parallel, up to 2), .ptx last (references audio files)
- **Version record in Convex created only after ALL files (including .ptx) are confirmed uploaded** — never before

### Multipart Upload

- Files <100 MiB: single PUT via presigned URL
- Files >= 100 MiB: multipart upload (100 MiB parts, up to 2 concurrent parts)
- Part progress tracked in SQLite for resume on failure
- On crash: resume from last successful part (R2 supports listing parts of incomplete uploads)

## File Watcher

- `notify::RecommendedWatcher` + `notify-debouncer-full` with per-file stability debounce
- **Stability check:** after debounce fires, poll file size every 2s. Upload only after 5s of no size change (handles active recordings)
- **Batch window:** collect changes for 10 seconds after first event, then process the whole batch
- Filter events through ignore patterns before processing
- Discard `EventKind::Access` events (reads, not writes)

## Auto-Push Cycle

While a session is checked out:

1. File watcher detects changes (debounced)
2. Stability check confirms files are done being written
3. Scanner diffs current state against SQLite
4. For new/modified files: request presigned URLs from Convex
5. Upload files via presigned URLs (respecting global concurrency limit)
6. Every 5 minutes: create version snapshot (manifest.json + Convex version record)
7. Update SQLite state

## Download Pipeline

- Request presigned download URLs from Convex (scoped to session)
- Stream file to `.sessionsync-tmp` temp file in the same directory
- On completion: verify BLAKE3 hash matches expected hash from manifest
- If hash matches: atomic rename from `.sessionsync-tmp` to final filename
- If hash mismatch: delete temp file, mark as error, retry next cycle
- Track download progress in SQLite for resume on interruption

## Periodic Full Scan

- Weekly: re-hash all local files, compare against SQLite state
- Catches anything the watcher missed (FSEvents overflow, external drive issues, manual file edits)
- Reconcile: upload any untracked changes, download any missing remote files
