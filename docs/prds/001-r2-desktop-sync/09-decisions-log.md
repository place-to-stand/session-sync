# 09: Decisions Log

**Dependencies:** None (standalone reference)

---

Every architectural decision made during the design process, with rationale.

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Sync model | Checkout + auto-push + release | Prevents conflicts by design. Scales to N users. |
| Spectator mode | Pull Latest (provisional) without checkout | Eliminates most common checkout friction ("can I just listen?") |
| Control plane | Convex (also credential broker) | Real-time subscriptions + presigned URL generation. Engineers never touch R2 keys. |
| Data plane | Cloudflare R2 | $0 egress, S3-compatible, cheap storage. |
| R2 credential model | Presigned URLs via Convex | No raw R2 keys on client machines. Fixes security, onboarding, and access control. |
| Desktop framework | Tauri 2 (Rust) | 30MB idle RAM. Rust learning curve accepted with extended timeline. |
| Credential storage | macOS Keychain | Not plaintext JSON. Convex auth token in Keychain. |
| Checkout safety | Heartbeat (5 min) + stale detection (30 min) | Crashed machines don't lock sessions forever. |
| Checkout atomicity | Convex conditional compare-and-swap | No race conditions on concurrent checkout attempts. |
| Server-side validation | All Convex mutations validate caller | `releaseSession` checks `checkedOutBy` matches caller. |
| Identity model | Users + Machines (linked) | Actions attributed to people, not just laptops. |
| Crash recovery | Write-ahead logging in SQLite | No half-done sync operations after crash. |
| Versioning | Auto-version + manifest + optional release note | Manifest required for rollback. Release note optional (auto-summary always shown). |
| Version format | manifest.json per version in R2 | File paths + BLAKE3 hashes. Without this, rollback is impossible. |
| R2 prefixes | UUIDs (not human names) | Prevents session name collisions. Human names in Convex only. |
| Global R2 manifest | Removed | Convex + SQLite are sources of truth. R2 listing never used for sync state. |
| Upload concurrency | Global semaphore, max 2 concurrent | Prevents saturating disk I/O and network during recording. |
| Upload priority | Checked-out session prioritized | Active work uploads before background sessions. |
| Hashing strategy | BLAKE3 mmap <1GB, streaming >1GB | Avoids memory pressure when Pro Tools is using RAM. |
| Delete behavior | Soft delete to `_trash/` prefix | Never lose work. Auto-purge after 30 days. |
| External drives | Supported with full scan on reconnect | Don't trust FSEvents across mount/unmount. |
| Schema compatibility | Version check on startup via Convex | Prevents silent client/server incompatibility. |
| Convex degraded mode | Cached state + queued mutations | Checkout blocked offline, auto-push continues with valid presigned URLs. |
| Multi-session | Day one (5+ sessions) | User manages many concurrent projects. Extended timeline accommodates. |
| Onboarding | Admin configures R2, invites partner via link | Invited engineer never sees R2 credentials. |
| Release notes | Optional (auto-summary always shown) | Reduces friction. Engineers won't write notes after 10-hour sessions. |
| Platform | macOS only (MVP) | Both engineers on Mac. Windows post-MVP. |
| Business model | Self-hosted MVP, SaaS later | Focus on making it work for us, figure out monetization later. |
| Timeline | 60-90 days (extended from 33) | Accounts for Rust learning curve and audit findings. |
| Pull behavior | Download only, no auto-open PT | Engineer opens Pro Tools themselves. |
| Pull vs Check Out | Separate actions | Pull downloads files (background). Check Out acquires lock (requires local copy). Can't check out until fully downloaded. |
| Version retention | Keep all versions, no pruning | Storage cost negligible (audio deduplicated, only .ptx snapshots add cost). Rollback to any point in history always possible. |
| Version snapshot frequency | Batch every 5 minutes | Files upload immediately for safety. Manifest + Convex version record created every 5 min. Prevents version churn (~120/day vs 300+). |
| Rust <-> Convex | HTTP API via reqwest | No Convex Rust SDK exists. HTTP POST to mutation/action/query endpoints. Batch presigned URL requests for multipart. |
| Invite mechanism | Signed token via Convex HTTP action | Single-use invite link, 7-day expiry. Invited engineer never sees R2 credentials. |
| Request notifications | Convex subscription -> Tauri event -> macOS notification | Advisory only, no automatic action. Stored in activity feed if recipient offline. |
| Session rename | Display name only (Convex) | R2 UUID prefix and .ptx filename unchanged. Prevents collisions and broken references. |
| Source of truth | Convex = state + metadata, R2 = files + manifests, SQLite = local cache | Three-layer hierarchy. Each layer authoritative for its domain. |
