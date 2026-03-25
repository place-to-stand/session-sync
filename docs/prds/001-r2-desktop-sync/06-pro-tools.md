# 06: Pro Tools Compatibility & Session Management

**Dependencies:** None

---

## Critical Pre-Implementation Validation

**Before writing the sync engine, validate that file watching a Pro Tools session folder does not interfere with Pro Tools itself.** Pro Tools is extremely sensitive to external filesystem changes during a session. Test:

1. Set up `notify` watcher on a session folder
2. Open the session in Pro Tools
3. Record audio, save the session, bounce, AudioSuite process
4. Verify Pro Tools does not show errors, lose audio, or behave unexpectedly

If file watching causes issues, this is a fundamental blocker that changes the architecture (would need to poll instead of watch).

## Pro Tools-Specific Behaviors

| Scenario | How SessionSync Handles It |
|----------|---------------------------|
| New recording (WAV growing) | Per-file size-stability check: poll size every 2s, upload only after 5s of no change |
| Session save (.ptx written) | Per-file debounce: coalesce burst writes, upload after 5s stable |
| AudioSuite destructive edit | Hash comparison detects modified WAV, re-uploads (write-once is perf optimization only) |
| Auto-backup to `Session File Backups/` | Ignored by default |
| `WaveCache.wfm` regenerated | Ignored |
| `.ptk` lock file appears | Informational only — Pro Tools has session open locally |
| Bounce/export (new large WAV) | Same as new recording — wait for stability, then push |
| Session on disconnected drive | Show warning, pause, full scan on reconnect |

## Sync Scope & Ignore Patterns

Everything in the session folder syncs **except** regenerable caches:

| Directory/File | Syncs? | Why |
|---------------|--------|-----|
| `Audio Files/` | Yes | Core content — recordings, takes |
| `Bounced Files/` | Yes | Rendered mixes, stems |
| `Video Files/` | Yes | Reference video, film scoring |
| `*.ptx` (session file) | Yes | Session layout, mix, automation |
| `Session File Backups/` | No | Pro Tools auto-backups, regenerable |
| `WaveCache.wfm` | No | Peak cache, regenerated on open |
| `*.pkf` (peak files) | No | Peak cache files, regenerated |
| `.DS_Store` | No | macOS metadata |
| `*.sessionsync-tmp` | No | Our temp files during downloads |

Custom ignore patterns configurable in Settings (gitignore-style syntax).

## Session Management

### Watching Sessions

Two methods, used together:

1. **Watch a parent directory** — e.g., `~/Documents/Pro Tools/`. SessionSync scans for folders containing `.ptx` files and registers them in Convex. New sessions appearing in the directory are auto-discovered.
2. **Add individual folders** — manually pick a session folder from any location, including external drives.

Session registry lives in Convex. All machines see the same session list. Each machine records its own local path for each session (via the `sessionMachines` junction table).

### External Drives

Sessions can live on external SSDs/HDDs. When a drive is disconnected:

- SessionSync detects the missing path
- Session shows "Drive disconnected" warning in the menu bar (persistent warning, no error spam)
- **On reconnect:** full directory scan + diff against SQLite state (don't trust FSEvents to have queued events during disconnect). Auto-resume and sync any changes.

### Archiving

- **Archive:** stop syncing, release any checkout, keep files in R2. Session moves to "Archived" section.
- **Unarchive:** resume syncing, session reappears in active list.
- **Cost:** R2 at $0.015/GB/month. A 20GB archived session costs $0.30/month.
