# 02: Sync Model — Checkout + Auto-Push + Release

**Dependencies:** None

---

Sessions use an explicit checkout/release cycle with automatic background sync while checked out, plus a spectator mode for read-only access.

## The Flow

```
┌─ PULL (background, independent of checkout) ─────────────┐
│                                                           │
│  Pull and Check Out are SEPARATE actions.                 │
│  You must pull a session before you can check it out.     │
│                                                           │
│  Two pull modes:                                          │
│                                                           │
│  "Pull Released Version" — downloads the last explicitly  │
│  released state. This is the canonical version.           │
│                                                           │
│  "Pull Latest (Provisional)" — downloads the most recent  │
│  auto-pushed snapshot, even if someone has it checked out. │
│  Clearly labeled as provisional. Read-only.               │
│  Use case: "I just want to hear the current mix."         │
│                                                           │
│  Pull runs in the background. Progress shown in the UI.   │
│  First pull of a session = full download (could be 80GB). │
│  Subsequent pulls = incremental (only new/changed files). │
│                                                           │
├─ CHECK OUT (requires local copy) ────────────────────────┤
│                                                           │
│  "Check Out" button only enabled when session is fully    │
│  downloaded locally AND not locked by someone else.       │
│                                                           │
│  → Atomic conditional mutation in Convex:                 │
│    SET checkedOutBy = me WHERE checkedOutBy IS NULL        │
│    (rejects if already checked out — no race condition)    │
│  → All other machines see lock within ~2 seconds           │
│  → File watcher starts on the local session folder         │
│                                                           │
├─ WHILE CHECKED OUT ──────────────────────────────────────┤
│                                                           │
│  Auto-push runs continuously in the background:           │
│  • File watcher detects changes (debounced for PT saves)  │
│  • New/modified files push to R2 via presigned URLs       │
│  • Version snapshots batched every 5 minutes              │
│    (files upload immediately, manifest created every 5m)  │
│  • Other engineers see "N versions since checkout"         │
│    but CANNOT check out the session                       │
│  • Other engineers CAN "spectator pull" the latest state  │
│                                                           │
│  Heartbeat: machine sends keepalive to Convex every 5 min │
│  If heartbeat missed for 30 min → session marked "stale"  │
│  Other engineer can then claim the session                │
│                                                           │
├─ RELEASE ────────────────────────────────────────────────┤
│                                                           │
│  Engineer clicks "Release"                                │
│  → Auto-generated summary shown: "8 new audio files,      │
│    .ptx modified, 2.3 GB uploaded"                        │
│  → Optional: add a human note ("rough mix v2, bass ODs")  │
│  → Final version snapshot created (marked as release)     │
│  → Lock released                                          │
│  → macOS notification + in-app badge sent to all machines │
│                                                           │
└──────────────────────────────────────────────────────────┘
```

## Session States

| State | Icon | Meaning | Available Actions |
|-------|------|---------|-------------------|
| Available (not local) | ⚪ | Nobody has it checked out, not downloaded | Pull Released, Pull Latest |
| Available (local) | ⚪ | Nobody has it checked out, files on disk | Pull Released, Pull Latest, **Check Out** |
| Checked out by you | 🟢 | You own it, auto-push active | Release |
| Updates available | 🔵 | Partner released new version | Pull Released, Pull Latest, **Check Out** (if local) |
| Checked out by other | 🟡 | Someone else is working | Pull Latest (provisional), Request |
| Stale checkout | 🟠 | Checked out but machine offline 30+ min | Claim Session |
| Pulling | ⏳ | Download in progress | Cancel Pull |
| Archived | 📦 | Stopped syncing, files in R2 | Unarchive |
| Drive disconnected | ⚠️ | External drive not mounted | — (auto-resumes on reconnect) |

**Key rule:** "Check Out" button is only enabled when the session is fully downloaded locally AND not locked by another machine. Pull and Check Out are independent actions.

## Checkout Safety Mechanisms

**Atomic checkout:** Convex mutation is a conditional compare-and-swap: `SET checkedOutBy = me WHERE checkedOutBy IS NULL`. Two engineers clicking "Check Out" at the same instant — one succeeds, one gets "already checked out." No race condition.

**Heartbeat-based stale detection:** While holding a checkout, the machine sends a heartbeat to Convex every 5 minutes. If heartbeats are missed for 30 minutes (configurable), the session is marked "stale checkout." Other engineers see 🟠 and can "Claim Session" — which releases the stale checkout and checks it out to them. The last auto-pushed state is preserved as the release point.

**Graceful shutdown:** On app quit, release any held checkouts. On crash/power loss, the heartbeat timeout handles it automatically.

**No force checkout needed:** The heartbeat-based stale detection covers the "crashed machine" case. The "Request" notification covers the "engineer went to lunch" case. Between these two mechanisms, sessions are never locked indefinitely.

## Versioning

- **Auto-push uploads files immediately** for safety (as soon as debounce clears), but **version snapshots are batched every 5 minutes.** This means individual files reach R2 in near-real-time, but a formal "version" (manifest + Convex record) is only created every 5 minutes during active work. This prevents version churn (a 10-hour session produces ~120 versions, not 300+).
- **Version manifest:** JSON at `_versions/{session_uuid}/v{N}/manifest.json` listing every file path + BLAKE3 hash at that point in time. The hash is the content-address for the underlying R2 object (e.g. `_objects/{blake3}`), required for rollback to work.
- **All versions retained** — no pruning. Storage cost is negligible: audio files are stored as **immutable, content-addressed objects** in R2, keyed by BLAKE3 hash, and are **never overwritten.** Any edit (including destructive in-place edits) produces a new object with a new hash/key. Deduplication happens when multiple versions reference the same hash, so each additional version typically only adds the .ptx snapshot (~50MB). At R2 pricing, 1,000 .ptx snapshots = ~50GB = $0.75/month.
- **Auto-generated summary per version:** "3 new audio files, .ptx modified" (computed from diff)
- **Optional release note:** shown in the release dialog, pre-filled with auto-summary. Human note is optional, not required.
- **Rollback:** pull any previous version using its manifest to determine exactly which immutable object hashes to restore for each file path. Since all manifests reference immutable content-addressed objects and all versions are retained, rollback to any point in history is always possible.

## Soft Delete

When an engineer deletes a file from a session folder:
- The file moves to `_trash/{session_uuid}/` prefix in R2 (uses UUID, not human name — consistent with all R2 prefixes)
- Recoverable through the UI or direct R2 access
- Trash auto-purges after 30 days (configurable)
