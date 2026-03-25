# PRD 001: SessionSync — R2 Desktop Sync for Recording Engineers

**Status:** Draft v4 (post-audit, ambiguities resolved)
**Author:** Jason Desiderio
**Created:** 2026-03-24
**Last Updated:** 2026-03-24

---

## Quick Summary

A macOS menu bar utility that manages Pro Tools session checkout, sync, and versioning through Cloudflare R2 (file storage) and Convex (real-time coordination + credential broker). Engineers explicitly check out sessions, work with auto-push in the background, then release when done. No conflicts by design.

**Timeline:** 60-90 days | **Platform:** macOS only (MVP) | **Cost:** ~$8.50/month for two engineers

---

## Document Map

Each document is self-contained and can be loaded independently into an LLM context window. Dependencies are listed at the top of each file.

```
README.md (this file)
│
├── 01-problem-and-solution.md    Why we're building this
│   No dependencies
│
├── 02-sync-model.md              The checkout/pull/release flow
│   No dependencies
│
├── 03-architecture.md            R2 + Convex layers, schema, security
│   ← reads: 02-sync-model.md (references session states)
│
├── 04-tech-stack.md              Technology choices, project structure, dependencies
│   ← reads: 03-architecture.md (references Convex schema)
│
├── 05-ui-spec.md                 Menu bar, setup wizard, invite flow
│   ← reads: 02-sync-model.md (references session states and actions)
│   ← reads: 03-architecture.md (references Convex, presigned URLs)
│
├── 06-pro-tools.md               PT compatibility, ignore patterns, session management
│   No dependencies
│
├── 07-sync-engine.md             WAL, upload pipeline, multipart, file watcher
│   ← reads: 03-architecture.md (references R2, presigned URLs)
│
├── 08-implementation-phases.md   7 phases, week-by-week plan
│   ← reads: ALL docs (references features from each)
│
├── 09-decisions-log.md           Every architectural decision + rationale
│   No dependencies (standalone reference)
│
├── 10-risks-and-future.md        Risks, mitigations, success metrics, cost, out of scope
│   No dependencies (standalone reference)
│
├── PROGRESS.md                   Updated after each coding session
│   ← reads: 08-implementation-phases.md (tracks against phases)
│
└── TEST-PLAN.md                  Manual test scenarios, updated after each session
    ← reads: 02-sync-model.md, 06-pro-tools.md, 07-sync-engine.md
```

### Which docs to load for common tasks

| Task | Load these docs |
|------|----------------|
| **Starting a new phase** | `08-implementation-phases.md` + `PROGRESS.md` + the relevant feature doc |
| **Building the sync engine** | `07-sync-engine.md` + `03-architecture.md` + `06-pro-tools.md` |
| **Building the UI** | `05-ui-spec.md` + `02-sync-model.md` |
| **Setting up Convex** | `03-architecture.md` + `04-tech-stack.md` |
| **Writing tests** | `TEST-PLAN.md` + the relevant feature doc |
| **Making an architecture decision** | `09-decisions-log.md` + the relevant feature doc |
| **Full context (rare)** | All docs (~1,000 lines total, fits in context) |
