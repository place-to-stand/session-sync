# 03: Architecture — R2 + Convex

**Dependencies:** Reads `02-sync-model.md` (references session states)

---

## Two-Layer Architecture

| Layer | Role | Responsibilities |
|-------|------|-----------------|
| **Cloudflare R2** (data plane) | File storage | Audio files, session files, version snapshots + manifests, soft-delete trash |
| **Convex** (control plane + credential broker) | Real-time coordination | Checkout state, version history, changeset notes, session registry, user/machine identity, activity feed, **presigned URL generation for R2 access** |

## Convex as Credential Broker (Key Security Architecture)

**No R2 credentials are stored on client machines.** Instead:

1. The admin (you) configures R2 credentials in Convex once (server-side, never exposed to clients)
2. When a client needs to upload or download, it requests a presigned URL from Convex
3. Convex validates the request (is this machine authorized? is the session checked out by them?) and returns a time-limited presigned URL scoped to the specific R2 object path
4. The client uses the presigned URL to upload/download directly to/from R2
5. Presigned URLs expire after 1 hour (configurable)

**This fixes three problems at once:**
- **Security:** No raw R2 keys on client machines. A stolen laptop can't access the bucket.
- **Onboarding:** New engineers only need a Convex invite, not R2 credentials.
- **Access control:** Convex can scope presigned URLs per-session, enforcing checkout ownership.

## Convex Degraded Mode

If Convex is unreachable:
- **Cache last-known state in SQLite:** session list, checkout status, version history
- **Display cached state with "offline" warning** in the menu bar
- **Block new checkouts** (can't guarantee exclusivity without Convex)
- **Continue auto-push for already-checked-out sessions** (presigned URLs are valid for 1 hour; request new ones when Convex returns)
- **Queue Convex mutations locally** (version records, activity events) and replay on reconnection
- **If offline > 1 hour:** presigned URLs expire. Auto-push pauses. Show "Waiting for connection" status. Resume automatically when Convex returns.

## R2 Bucket Organization

Single bucket, prefixed by session:

```
session-sync-bucket/
├── sessions/
│   ├── {uuid-rivera-album}/
│   │   ├── Audio Files/Audio 1_01.wav
│   │   ├── Audio Files/Audio 2_01.wav
│   │   ├── Bounced Files/Rough Mix.wav
│   │   ├── Video Files/Reference.mov
│   │   └── Rivera Album.ptx
│   ├── {uuid-smith-ep}/
│   │   └── ...
│   └── {uuid-garcia-single}/
│       └── ...
├── _versions/
│   ├── {uuid-rivera-album}/
│   │   ├── v001/
│   │   │   ├── manifest.json          # File paths + BLAKE3 hashes at this point
│   │   │   └── Rivera Album.ptx       # Snapshot of session file
│   │   ├── v002/
│   │   │   ├── manifest.json
│   │   │   └── Rivera Album.ptx
│   │   └── ...
│   └── ...
└── _trash/
    ├── {uuid-rivera-album}/
    │   └── deleted-file.wav
    └── ...
```

**Note:** Session prefixes use UUIDs (not human-readable names) to prevent collisions. Human-readable names are stored in Convex only. Renaming a session in SessionSync changes the Convex display name only — the R2 UUID prefix and the .ptx filename are unchanged.

**Sources of truth:**
- **Convex** = session state (checkout, users, machines), version metadata (who pushed, when, notes), and session registry (name <-> UUID mapping)
- **R2** = file contents and version manifests (the manifest.json per version is the definitive record of which files at which hashes constitute that version)
- **SQLite** = local cache of both, plus file hashing state and WAL entries for crash recovery

## Convex Schema

```
users
  - id: Id<"users">
  - name: string                       # "Jason Desiderio"
  - email: string                      # For identity, not auth (MVP uses Convex anonymous auth)
  - createdAt: number

machines
  - id: Id<"machines">
  - userId: Id<"users">               # Which person owns this machine
  - machineId: string                  # "austin-macbook" (stable local ID)
  - displayName: string                # "Austin Studio"
  - lastHeartbeatAt: number            # For stale checkout detection
  - appVersion: string                 # For schema version compatibility check
  - platform: string                   # "macos"

sessions
  - id: Id<"sessions">
  - name: string                       # "Rivera Album"
  - r2Prefix: string                   # "sessions/{uuid}"
  - checkedOutBy: Id<"machines"> | null
  - checkedOutAt: number | null
  - lastHeartbeatAt: number | null     # From the machine holding checkout
  - status: "available" | "checked_out" | "stale" | "archived"
  - createdAt: number
  - updatedAt: number

sessionMachines                        # Which machines watch which sessions
  - id: Id<"sessionMachines">
  - sessionId: Id<"sessions">
  - machineId: Id<"machines">
  - localPath: string                  # "/Volumes/External/Pro Tools/Rivera Album"

versions
  - id: Id<"versions">
  - sessionId: Id<"sessions">
  - versionNumber: number
  - pushedBy: Id<"machines">
  - autoSummary: string                # "3 new audio files, .ptx modified"
  - releaseNote: string | null         # null for auto-versions, populated on release
  - isRelease: boolean                 # true = explicit release, false = auto-push
  - filesChanged: number
  - bytesChanged: number
  - r2ManifestKey: string              # "_versions/{uuid}/v{N}/manifest.json"
  - createdAt: number

activity
  - id: Id<"activity">
  - sessionId: Id<"sessions">
  - machineId: Id<"machines">
  - action: "checkout" | "release" | "pull" | "push" | "archive" | "request" | "claim"
  - details: string | null
  - createdAt: number
```

## Server-Side Validation (Convex Mutations)

All mutations enforce authorization:

- **`checkoutSession`**: atomic conditional — rejects if `checkedOutBy` is not null (unless status is "stale")
- **`releaseSession`**: validates that the requesting machine matches `checkedOutBy`
- **`claimSession`**: only allowed when session status is "stale" (heartbeat expired)
- **`requestPresignedUrl`**: validates the machine is registered, scopes URL to the requested session path. Upload URLs only issued if the machine holds the checkout for that session. Read-only (download) URLs issued to any registered machine.
- **`createVersion`**: only accepted from the machine holding the checkout

## Rust <-> Convex Communication

Convex has no official Rust SDK. The Rust sync engine communicates with Convex via **HTTP API** directly using `reqwest`:

- **Mutations** (checkout, release, heartbeat, create version): `POST https://<deployment>.convex.cloud/api/mutation` with function name + args JSON
- **Actions** (presigned URL generation): `POST https://<deployment>.convex.cloud/api/action` — actions can call external services (R2 presigned URL signing)
- **Queries** (session state, version list): `POST https://<deployment>.convex.cloud/api/query` for one-shot reads

The **React frontend** uses the official Convex JS client for real-time subscriptions (session state changes, activity feed). The **Rust backend** uses HTTP for all operational calls (presigned URLs, heartbeats, version creation).

**Presigned URL batching for multipart uploads:** For a 10GB file (100 parts), the Rust client calls a single Convex action `requestMultipartPresignedUrls(sessionId, filePath, numParts)` which returns all part URLs + the create/complete URLs in one round trip.

## Schema Version Compatibility

Each machine reports its `appVersion` in the heartbeat. Convex stores a `minClientVersion` config value. If a client's version is below the minimum, Convex mutations return an "update required" error, and the app shows a "Please update SessionSync" message with a download link. This prevents silent incompatibility after schema changes.
