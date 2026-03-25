import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export default defineSchema({
  // Registered users (people, not machines)
  users: defineTable({
    name: v.string(),
    email: v.string(),
    role: v.union(v.literal("admin"), v.literal("engineer")),
    createdAt: v.number(),
  })
    .index("by_email", ["email"]),

  // Physical machines linked to users
  machines: defineTable({
    userId: v.id("users"),
    machineId: v.string(), // Stable local ID, e.g. "austin-macbook"
    displayName: v.string(), // Human-friendly, e.g. "Austin Studio"
    lastHeartbeatAt: v.number(),
    appVersion: v.string(), // For schema version compatibility check
    platform: v.string(), // "macos", "windows", etc.
    createdAt: v.number(),
  })
    .index("by_machineId", ["machineId"])
    .index("by_userId", ["userId"]),

  // Pro Tools sessions tracked by the system
  sessions: defineTable({
    name: v.string(), // Human-friendly: "Rivera Album"
    r2Prefix: v.string(), // "sessions/{uuid}" — immutable after creation
    checkedOutBy: v.optional(v.id("machines")), // undefined when available
    checkedOutAt: v.optional(v.number()),
    lastHeartbeatAt: v.optional(v.number()), // From the machine holding checkout
    status: v.union(
      v.literal("available"),
      v.literal("checked_out"),
      v.literal("stale"),
      v.literal("archived"),
    ),
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index("by_status", ["status"])
    .index("by_checkedOutBy", ["checkedOutBy"]),

  // Junction: which machines watch which sessions (and where locally)
  sessionMachines: defineTable({
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    localPath: v.string(), // "/Volumes/External/Pro Tools/Rivera Album"
  })
    .index("by_session", ["sessionId"])
    .index("by_machine", ["machineId"])
    .index("by_session_machine", ["sessionId", "machineId"]),

  // Version snapshots (auto-push every 5 min + explicit releases)
  versions: defineTable({
    sessionId: v.id("sessions"),
    versionNumber: v.number(),
    pushedBy: v.id("machines"),
    autoSummary: v.string(), // "3 new audio files, .ptx modified"
    releaseNote: v.optional(v.string()), // null for auto-versions
    isRelease: v.boolean(), // true = explicit release, false = auto-push
    filesChanged: v.number(),
    bytesChanged: v.number(),
    r2ManifestKey: v.string(), // "_versions/{uuid}/v{N}/manifest.json"
    createdAt: v.number(),
  })
    .index("by_session", ["sessionId"])
    .index("by_session_version", ["sessionId", "versionNumber"])
    .index("by_session_release_version", ["sessionId", "isRelease", "versionNumber"]),

  // Activity feed for all session events
  activity: defineTable({
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    action: v.union(
      v.literal("checkout"),
      v.literal("release"),
      v.literal("pull"),
      v.literal("push"),
      v.literal("archive"),
      v.literal("unarchive"),
      v.literal("request"),
      v.literal("claim"),
    ),
    details: v.optional(v.string()),
    createdAt: v.number(),
  })
    .index("by_session", ["sessionId"])
    .index("by_session_created", ["sessionId", "createdAt"])
    .index("by_createdAt", ["createdAt"]),

  // Single-use invite links with HMAC-signed tokens
  invites: defineTable({
    createdBy: v.id("users"),
    email: v.string(), // Invited person's email
    name: v.string(), // Invited person's name
    role: v.union(v.literal("admin"), v.literal("engineer")),
    token: v.string(), // HMAC-signed token
    expiresAt: v.number(), // 7 days from creation
    redeemedAt: v.optional(v.number()),
    redeemedBy: v.optional(v.id("users")),
    revokedAt: v.optional(v.number()),
    createdAt: v.number(),
  })
    .index("by_token", ["token"])
    .index("by_email", ["email"]),

  // Global configuration (minClientVersion, etc.)
  config: defineTable({
    key: v.string(),
    value: v.string(), // JSON-encoded value for flexibility
    updatedAt: v.number(),
  })
    .index("by_key", ["key"]),
});
