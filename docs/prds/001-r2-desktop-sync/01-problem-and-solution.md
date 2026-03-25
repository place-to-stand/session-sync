# 01: Problem & Solution

**Dependencies:** None

---

## Problem Statement

Google Drive Desktop is unreliable for syncing Pro Tools sessions between recording engineers across locations. Files unlink, uploads stall, and there's no awareness of DAW-specific file patterns. This costs time, causes frustration during active sessions, and risks losing work.

Beyond the immediate two-person workflow, studios with multiple engineers across locations have no good solution for managing session ownership, versioning, and handoffs of large binary creative assets.

## Proposed Solution

A macOS menu bar utility that manages Pro Tools session checkout, sync, and versioning through Cloudflare R2 (file storage) and Convex (real-time coordination + credential broker). Engineers explicitly check out sessions, work with auto-push running in the background, then release with optional changeset notes when done. Other engineers see session status in real-time and can pull the latest version — or grab a read-only "spectator" snapshot without checking out.

## Why This Model?

**Real-time sync (Dropbox model) is wrong for audio.** You save your .ptx 50 times during a mixing session. Each save pushes to your partner — they see half-finished mixes, broken automation, incomplete routing. At studio scale (10+ engineers), it's chaos.

**Git doesn't work either.** .ptx files are binary (can't merge, can't diff). Audio files are huge. Recording engineers won't learn `git commit`.

**The checkout model is how professional media workflows operate** — Avid MediaCentral, Perforce (game studios), Frame.io. Clear ownership, intentional handoffs, no conflicts by design.

## Why R2?

- **$0 egress** — pulling down 500GB of sessions per month costs nothing (vs. real money on S3/GCS)
- **S3-compatible API** — mature tooling, well-documented
- **$0.015/GB/month storage** — a 1TB session archive costs $15/month
- No vendor lock-in to Google/Dropbox ecosystems

## Key Insight

Pro Tools audio files are **write-once** — new recordings always create new WAV files; existing WAVs are never modified. Only the small `.ptx` session file gets edited. This means audio files can auto-sync safely in the background (zero conflict risk), while the .ptx uses the checkout model for clear ownership.

**Caveat:** AudioSuite destructive editing can modify WAV files in place. The sync engine treats write-once as a *performance optimization* (skip re-hashing unchanged files) but never relies on it for *correctness*. Modified WAVs are detected via hash comparison and handled normally.

## Competitive Landscape

| Product | Model | Pro Tools? | Checkout? | Cost |
|---------|-------|-----------|-----------|------|
| **Google Drive** | Real-time bidirectional sync | No DAW awareness | No | $10-20/mo per user |
| **Dropbox** | Real-time bidirectional sync | No DAW awareness | No | $12-20/mo per user |
| **Splice** | Git-like push/pull versioning | Ableton & Logic only | No | $8-14/mo |
| **Avid Cloud Collab** | Real-time within Pro Tools | Yes (built-in) | Partial | Requires Avid subscription |
| **Gobbler** | Cloud backup for sessions | Yes | No | Defunct (reliability + pricing issues) |
| **Soundwhale** | Real-time audio streaming | N/A (live collaboration) | N/A | $30+/mo |
| **SessionSync** | Checkout + auto-push + release | Yes (DAW-aware) | Yes (explicit) | R2 storage only (~$15/TB/mo) |

**Our differentiator:** The combination of (1) Pro Tools awareness (write-once audio insight), (2) explicit checkout preventing conflicts by design, (3) R2's zero-egress economics, and (4) real-time status visibility via Convex. No competitor has all four.

## Target Users

**MVP:** Two recording engineers (Austin + NYC) collaborating on Pro Tools sessions remotely, managing 5+ active sessions across internal and external drives.

**Long-term:** Multi-location recording studios with 10+ engineers, and any company that makes large media files (audio, video, photo) needing reliable cloud sync.
