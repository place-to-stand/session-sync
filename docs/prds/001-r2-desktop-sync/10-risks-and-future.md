# 10: Risks, Cost, Success Metrics & Future

**Dependencies:** None (standalone reference)

---

## Risks & Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| Pro Tools rejects file watching | Critical | Validate BEFORE writing sync engine (Phase 1, day 1) |
| Rust learning curve extends timeline | High | 60-90 day timeline budgeted. Sync engine built as standalone lib, testable without Tauri. |
| Convex outage blocks all coordination | High | Cached state in SQLite, queued mutations, auto-push continues with valid presigned URLs |
| Trust destroyed by one corrupted session | High | BLAKE3 hash verification on every download. WAL for crash recovery. Weekly integrity scan. |
| Presigned URL expiry during large upload | Medium | Request new URLs mid-upload if approaching expiry. Multipart upload parts use individual presigned URLs. |
| R2 multipart behavior differs from S3 | Medium | Test multipart abort/resume specifically against R2. Document any R2-specific workarounds. |
| Menu bar panel UX is laggy | Medium | Prototype tray popover in Phase 1. Fix before proceeding. |
| FSEvents misses events on external drives | Medium | Full directory scan on drive reconnect. Never trust FSEvents across mount/unmount. |
| Auto-push competes with Pro Tools for I/O | Medium | Global upload semaphore (max 2). Priority queue for checked-out session. |
| Engineer abandons checkout (goes home) | Medium | Heartbeat-based stale detection (30 min). Other engineer can claim. |

## Cost Analysis

### MVP (Two Engineers, ~500GB total)

| Item | Monthly Cost |
|------|-------------|
| R2 storage (500 GB) | $7.50 |
| R2 operations (~100K writes, ~500K reads) | ~$1.00 |
| R2 egress | $0.00 |
| Convex free tier | $0.00 |
| **Total** | **~$8.50/month** |

vs. Google One 2TB: $10/month/user ($20 total for two users)
vs. Dropbox Plus: $12/month/user ($24 total)

### Studio Scale (10 Engineers, ~5TB total)

| Item | Monthly Cost |
|------|-------------|
| R2 storage (5 TB) | $75.00 |
| R2 operations | ~$10.00 |
| Convex Pro plan (estimated) | ~$25.00 |
| **Total** | **~$110/month** |

vs. Google Workspace Business: $12/user/month ($120 total, 5TB pooled)
vs. Dropbox Business: $20/user/month ($200 total)

## Verification Plan

1. **Pre-implementation:** Pro Tools + file watcher compatibility test (BLOCKER)
2. **Unit tests** (Rust): scanner change detection, ignore patterns, hash caching, WAL replay, file stability check
3. **Unit tests** (Convex): checkout/release mutations (atomic conditional), concurrent checkout rejection, stale detection, presigned URL scoping
4. **Integration tests** (Rust): R2 upload/download via presigned URLs, multipart upload/resume, soft-delete
5. **Integration tests** (Convex): real-time subscription propagation, credential broker flow, schema version check
6. **Manual end-to-end**: Two machines — full checkout -> work -> auto-push -> release -> pull cycle. Plus spectator pull.
7. **Stress test**: 50GB session, verify sync completes, all hashes match
8. **Failure scenarios**: Network disconnect mid-upload, app crash (WAL replay), Convex down (cached state), disk full (pre-check), external drive disconnect/reconnect (full scan)
9. **Security test**: Verify presigned URLs expire, verify Convex mutations reject unauthorized callers, verify no R2 credentials on client disk

## Success Metrics

- **Reliability:** Zero files lost or corrupted over 30 days of daily use
- **Sync speed:** New audio file (100MB WAV) pushed to R2 within 60 seconds of recording completion
- **Checkout latency:** Checkout/release state visible to other machines within 2 seconds
- **Pull experience:** 20GB session pull completes in under 20 minutes on 100Mbps+ connection, with accurate ETA displayed
- **Resource usage:** <50MB idle RAM, <5% CPU when idle, no interference with Pro Tools
- **Onboarding:** Invited engineer connected and syncing within 10 minutes (no R2 credentials needed)
- **Cost:** Total infrastructure cost <$10/month for two-person MVP

## Out of Scope (Post-MVP)

- Windows support
- Web dashboard for remote status viewing
- Logic Pro (.logicx) and Ableton (.als) support
- Guest engineer / scoped session access
- Client review links (shareable presigned URLs to bounced files)
- Slack/iMessage integration for request notifications
- Bandwidth throttling (fast internet assumed for MVP)
- SaaS hosted offering (self-hosted first)
- Admin panel for studio management
- Auto-update mechanism (version check warning only for MVP)
- Selective sync (download only specific subdirectories)
