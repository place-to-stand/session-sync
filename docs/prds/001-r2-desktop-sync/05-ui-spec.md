# 05: UI Specification

**Dependencies:** Reads `02-sync-model.md` (session states and actions), `03-architecture.md` (Convex, presigned URLs)

---

## Menu Bar Panel

```
┌─────────────────────────────────────────┐
│  SessionSync                    ⚙       │
├─────────────────────────────────────────┤
│                                         │
│  ACTIVE SESSIONS                        │
│                                         │
│  🟢 Rivera Album           [Release]   │
│     ● Synced — 4 new audio files pushed │
│     Last saved 3 min ago                │
│                                         │
│  🔵 Smith EP               [Pull ▾]    │
│     NYC released 12 min ago             │
│     "rough mix v3, bass overdubs"       │
│     ┌─────────────────────────┐         │
│     │ Pull Released Version   │         │
│     │ Pull Latest (Prov.)     │         │
│     │ Check Out               │         │
│     └─────────────────────────┘         │
│                                         │
│  🟡 Garcia Single (NYC)    [Pull ▾]    │
│     Checked out by nyc-studio           │
│     Since 2:30 PM · 5 auto-pushes      │
│     ┌─────────────────────────┐         │
│     │ Pull Latest (Prov.)     │         │
│     │ Request                 │         │
│     └─────────────────────────┘         │
│                                         │
│  🟠 Blues Project (NYC)     [Claim]    │
│     Stale — NYC offline 45 min          │
│                                         │
│  ⚠️ Demo Rough Mix           [—]       │
│     Drive disconnected                  │
│                                         │
├─────────────────────────────────────────┤
│  ACTIVITY                               │
│  NYC released Smith EP · 12 min ago     │
│  You pushed Rivera Album · 3 min ago    │
│  NYC checked out Garcia · 2:30 PM       │
├─────────────────────────────────────────┤
│  📦 Archived (3)                    ▸   │
│  + Add Session                      ▸   │
│  ⚙ Settings                         ▸   │
└─────────────────────────────────────────┘
```

## Sync Backlog Indicator

During active recording/work, the session card shows sync status:

- **"● Synced"** — auto-push is caught up, no pending files
- **"● Pushing 3 files (1.2 GB remaining)"** — actively uploading
- **"● 12 GB queued"** — backlog during heavy recording

## Pull Progress UI

Pulling a large session (20-80GB) is the most visible operation. The pull UI shows:

- Overall progress bar with ETA ("Pulling Smith EP — 14.2 / 23.1 GB — ~8 min remaining")
- Per-file progress for the current download
- Ability to cancel (confirms first: "Stop pull? Downloaded files will be kept.")
- Resume from where you left off (multipart download tracking in SQLite)

## Release Dialog

```
┌─────────────────────────────────────────┐
│  Release Rivera Album                   │
├─────────────────────────────────────────┤
│                                         │
│  Changes since checkout:                │
│  • 8 new audio files (2.3 GB)           │
│  • Session file modified                │
│  • 1 bounced file added                 │
│  • 12 auto-pushes over 4 hours          │
│                                         │
│  Add a note (optional):                 │
│  ┌─────────────────────────────────┐    │
│  │ rough mix v2, added bass ODs   │    │
│  └─────────────────────────────────┘    │
│                                         │
│         [Cancel]    [Release]           │
└─────────────────────────────────────────┘
```

Release note is **optional**. Auto-summary is always shown.

## Tray Icon States

- **Idle** — gray icon, no activity
- **Syncing** — blue/animated, uploads or downloads in progress
- **Attention** — orange badge, stale checkout or new release available
- **Error** — red, persistent failures

---

## Setup Wizard (First-Run Experience)

### Admin Flow (First Machine)

```
Step 1: Welcome
  "SessionSync keeps your Pro Tools sessions in sync across studios."
  [Get Started]

Step 2: Your Identity
  Name: [Jason Desiderio]
  Machine Name: [Austin Studio]  (pre-filled from hostname)
  [Next]

Step 3: Connect Storage
  "SessionSync uses Cloudflare R2 for file storage."
  Account ID: [________________]
  Access Key ID: [________________]
  Secret Access Key: [________________]
  Bucket Name: [session-sync]
  [Test Connection ✓]  [Next]

Step 4: Add Your First Session
  [Watch a folder] — auto-detect Pro Tools sessions in a directory
  [Add a session] — pick a specific session folder
  [Skip for now]

Step 5: Invite Your Partner
  "Share this link with your partner to connect their machine:"
  [https://sessionsync.convex.site/invite/abc123] [Copy]
  "They won't need R2 credentials — SessionSync handles that."
  [Done]
```

### Invited Engineer Flow

```
Step 1: Welcome
  "Jason invited you to SessionSync."
  [Get Started]

Step 2: Your Identity
  Name: [________________]
  Machine Name: [NYC Studio]  (pre-filled from hostname)
  [Next]

Step 3: Open Invite Link
  Paste invite link: [https://sessionsync.convex.site/invite/abc123]
  [Connect ✓]

Step 4: Choose Session Folder
  "Where should sessions be stored on this machine?"
  [Choose Folder]  →  ~/Documents/Pro Tools/
  [Done — Start Syncing]
```

**The invited engineer never sees R2 credentials.**

### Admin Flow (Detailed Validation)

| Step | Screen | Fields | Validation |
|------|--------|--------|------------|
| 1 | Welcome | — | — |
| 2 | Identity | Name (text), Machine Name (text, pre-filled from hostname) | Both required, machine name unique |
| 3 | R2 Storage | Account ID, Access Key ID, Secret Access Key, Bucket Name | "Test Connection" button verifies credentials + bucket access |
| 4 | First Session | "Watch a folder" or "Add a session" or "Skip" | Folder picker, validates `.ptx` exists if adding specific session |
| 5 | Invite Partner | Generated invite link + "Copy" button | Link auto-expires after 7 days |

---

## Invite Link Mechanism

The invite link (`https://<deployment>.convex.site/invite/{token}`) is generated by a Convex HTTP action:

1. Admin clicks "Invite Partner" -> Tauri calls Convex mutation `createInvite()` -> returns a signed token (UUID + HMAC signature, expires in 7 days)
2. Admin shares the link (copy/paste, iMessage, email)
3. Invited engineer pastes the link into the SessionSync setup wizard
4. The desktop app extracts the token and calls Convex mutation `redeemInvite(token, machineName, userName)` -> validates signature + expiry -> creates user + machine records -> returns Convex auth credentials
5. Auth credentials stored in macOS Keychain. Machine is now registered.

**Security note for MVP:** Invites are single-use-ish (one invite per person, but not strictly enforced). For the two-person MVP, this is sufficient.

## Request Notification Mechanism

When an engineer clicks "Request" on a session checked out by someone else:

1. Convex mutation `requestSession(sessionId)` creates an activity record
2. The checkout holder's machine receives this via Convex real-time subscription (React frontend)
3. Frontend emits a Tauri event -> Rust backend triggers a macOS notification: "NYC Studio is requesting Rivera Album"
4. The notification is advisory — no automatic action. The checkout holder decides when to release.
5. If the checkout holder is offline, the request is stored in Convex. They see it when they come back online (activity feed shows "NYC Studio requested Rivera Album - 2h ago").
