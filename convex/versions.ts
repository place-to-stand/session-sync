import { v } from "convex/values";
import { mutation, query } from "./_generated/server";
import { Doc, Id } from "./_generated/dataModel";

/**
 * Create a new version snapshot for a session. Only the machine that
 * currently holds the checkout can create versions.
 *
 * The Rust sync engine calls this after uploading all files + manifest to R2.
 * A version record is NEVER created before the files are confirmed uploaded.
 */
export const createVersion = mutation({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    autoSummary: v.string(),
    releaseNote: v.optional(v.string()),
    isRelease: v.boolean(),
    filesChanged: v.number(),
    bytesChanged: v.number(),
    r2ManifestKey: v.string(),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    // Only the checkout holder can push versions
    if (session.checkedOutBy !== args.machineId) {
      const holder = session.checkedOutBy
        ? (await ctx.db.get(session.checkedOutBy))?.displayName ?? "another machine"
        : "nobody";
      throw new Error(
        `Cannot create version: session "${session.name}" is checked out by ${holder}, ` +
        `not by the requesting machine.`
      );
    }

    // Verify machine exists
    const machine = await ctx.db.get(args.machineId);
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }

    // Determine next version number by querying the highest existing version
    const latestVersion = await ctx.db
      .query("versions")
      .withIndex("by_session_version", (q) =>
        q.eq("sessionId", args.sessionId)
      )
      .order("desc")
      .first();

    const versionNumber = latestVersion ? latestVersion.versionNumber + 1 : 1;

    const now = Date.now();
    const versionId = await ctx.db.insert("versions", {
      sessionId: args.sessionId,
      versionNumber,
      pushedBy: args.machineId,
      autoSummary: args.autoSummary,
      releaseNote: args.releaseNote,
      isRelease: args.isRelease,
      filesChanged: args.filesChanged,
      bytesChanged: args.bytesChanged,
      r2ManifestKey: args.r2ManifestKey,
      createdAt: now,
    });

    // Update session timestamp
    await ctx.db.patch(args.sessionId, {
      updatedAt: now,
    });

    return { versionId, versionNumber };
  },
});

/**
 * List all versions for a session, ordered by version number descending
 * (newest first).
 */
export const listVersions = query({
  args: {
    sessionId: v.id("sessions"),
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    let query = ctx.db
      .query("versions")
      .withIndex("by_session_version", (q) =>
        q.eq("sessionId", args.sessionId)
      )
      .order("desc");

    if (args.limit) {
      return await query.take(args.limit);
    }
    return await query.collect();
  },
});

/**
 * Get the latest release version for a session (isRelease === true).
 * This is what engineers pull when they select "Pull Released Version".
 */
export const getLatestRelease = query({
  args: {
    sessionId: v.id("sessions"),
  },
  handler: async (ctx, args) => {
    // Get all versions for this session in descending order
    const versions = await ctx.db
      .query("versions")
      .withIndex("by_session_version", (q) =>
        q.eq("sessionId", args.sessionId)
      )
      .order("desc")
      .collect();

    // Find the first (most recent) release version
    const latestRelease = versions.find((v) => v.isRelease);
    return latestRelease ?? null;
  },
});

/**
 * Get the latest version of any kind (auto-push or release) for a session.
 * This is what engineers pull when they select "Pull Latest (Provisional)".
 */
export const getLatestVersion = query({
  args: {
    sessionId: v.id("sessions"),
  },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("versions")
      .withIndex("by_session_version", (q) =>
        q.eq("sessionId", args.sessionId)
      )
      .order("desc")
      .first();
  },
});

/**
 * Get a specific version by its document ID.
 */
export const getVersion = query({
  args: {
    versionId: v.id("versions"),
  },
  handler: async (ctx, args) => {
    const version = await ctx.db.get(args.versionId);
    if (!version) {
      throw new Error(`Version ${args.versionId} not found`);
    }
    return version;
  },
});

/**
 * Get a specific version by session + version number.
 */
export const getVersionByNumber = query({
  args: {
    sessionId: v.id("sessions"),
    versionNumber: v.number(),
  },
  handler: async (ctx, args) => {
    const version = await ctx.db
      .query("versions")
      .withIndex("by_session_version", (q) =>
        q.eq("sessionId", args.sessionId).eq("versionNumber", args.versionNumber)
      )
      .unique();

    if (!version) {
      throw new Error(
        `Version ${args.versionNumber} not found for session ${args.sessionId}`
      );
    }
    return version;
  },
});
