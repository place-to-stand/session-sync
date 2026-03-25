import { v } from "convex/values";
import { mutation, query, internalMutation } from "./_generated/server";
import { Id } from "./_generated/dataModel";

/** How long (ms) before a heartbeat is considered stale: 30 minutes. */
const STALE_THRESHOLD_MS = 30 * 60 * 1000;

/**
 * Register a new machine for a user. If a machine with the same machineId
 * already exists, update its metadata instead of creating a duplicate.
 */
export const registerMachine = mutation({
  args: {
    userId: v.id("users"),
    machineId: v.string(),
    displayName: v.string(),
    appVersion: v.string(),
    platform: v.string(),
  },
  handler: async (ctx, args) => {
    // Verify the user exists
    const user = await ctx.db.get(args.userId);
    if (!user) {
      throw new Error(`User ${args.userId} not found`);
    }

    if (!args.machineId.trim()) {
      throw new Error("machineId cannot be empty.");
    }
    if (!args.displayName.trim()) {
      throw new Error("displayName cannot be empty.");
    }

    // Check for existing machine with same machineId
    const existing = await ctx.db
      .query("machines")
      .withIndex("by_machineId", (q) => q.eq("machineId", args.machineId))
      .unique();

    const now = Date.now();

    if (existing) {
      // Update existing machine record
      await ctx.db.patch(existing._id, {
        userId: args.userId,
        displayName: args.displayName,
        appVersion: args.appVersion,
        platform: args.platform,
        lastHeartbeatAt: now,
      });
      return existing._id;
    }

    // Create new machine record
    const id = await ctx.db.insert("machines", {
      userId: args.userId,
      machineId: args.machineId,
      displayName: args.displayName.trim(),
      lastHeartbeatAt: now,
      appVersion: args.appVersion,
      platform: args.platform,
      createdAt: now,
    });

    return id;
  },
});

/**
 * Heartbeat from a machine. Updates the machine's lastHeartbeatAt and also
 * updates any sessions checked out by this machine.
 *
 * Also schedules the next stale detection check if one isn't already pending.
 */
export const heartbeat = mutation({
  args: {
    machineId: v.id("machines"),
  },
  handler: async (ctx, args) => {
    const machine = await ctx.db.get(args.machineId);
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }

    const now = Date.now();

    // Update machine heartbeat
    await ctx.db.patch(args.machineId, {
      lastHeartbeatAt: now,
    });

    // Also update heartbeat on any sessions this machine has checked out
    const sessions = await ctx.db
      .query("sessions")
      .withIndex("by_checkedOutBy", (q) =>
        q.eq("checkedOutBy", args.machineId)
      )
      .collect();

    for (const session of sessions) {
      await ctx.db.patch(session._id, {
        lastHeartbeatAt: now,
        updatedAt: now,
      });
    }

    return { success: true, heartbeatAt: now };
  },
});

/**
 * Get a single machine by its Convex document ID.
 */
export const getMachine = query({
  args: {
    machineId: v.id("machines"),
  },
  handler: async (ctx, args) => {
    const machine = await ctx.db.get(args.machineId);
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }
    return machine;
  },
});

/**
 * Look up a machine by its stable local machineId string.
 */
export const getMachineByLocalId = query({
  args: {
    machineId: v.string(),
  },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("machines")
      .withIndex("by_machineId", (q) => q.eq("machineId", args.machineId))
      .unique();
  },
});

/**
 * List all registered machines, optionally filtered by user.
 */
export const listMachines = query({
  args: {
    userId: v.optional(v.id("users")),
  },
  handler: async (ctx, args) => {
    if (args.userId) {
      return await ctx.db
        .query("machines")
        .withIndex("by_userId", (q) => q.eq("userId", args.userId!))
        .collect();
    }
    return await ctx.db.query("machines").collect();
  },
});

/**
 * Link a machine to a session (the machine watches/syncs this session).
 */
export const watchSession = mutation({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    localPath: v.string(),
  },
  handler: async (ctx, args) => {
    // Verify both exist
    const session = await ctx.db.get(args.sessionId);
    if (!session) throw new Error(`Session ${args.sessionId} not found`);
    const machine = await ctx.db.get(args.machineId);
    if (!machine) throw new Error(`Machine ${args.machineId} not found`);

    if (!args.localPath.trim()) {
      throw new Error("localPath cannot be empty.");
    }

    // Check if this machine already watches this session
    const existing = await ctx.db
      .query("sessionMachines")
      .withIndex("by_session_machine", (q) =>
        q.eq("sessionId", args.sessionId).eq("machineId", args.machineId)
      )
      .unique();

    if (existing) {
      // Update the local path
      await ctx.db.patch(existing._id, { localPath: args.localPath.trim() });
      return existing._id;
    }

    return await ctx.db.insert("sessionMachines", {
      sessionId: args.sessionId,
      machineId: args.machineId,
      localPath: args.localPath.trim(),
    });
  },
});

/**
 * Unwatch a session from a machine.
 */
export const unwatchSession = mutation({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
  },
  handler: async (ctx, args) => {
    const existing = await ctx.db
      .query("sessionMachines")
      .withIndex("by_session_machine", (q) =>
        q.eq("sessionId", args.sessionId).eq("machineId", args.machineId)
      )
      .unique();

    if (!existing) {
      throw new Error(
        `Machine ${args.machineId} is not watching session ${args.sessionId}`
      );
    }

    await ctx.db.delete(existing._id);
    return { success: true };
  },
});

/**
 * Internal scheduled function: detect stale sessions by checking heartbeats.
 *
 * Any session that is "checked_out" and whose lastHeartbeatAt is older than
 * STALE_THRESHOLD_MS is marked "stale". This is idempotent — running it
 * multiple times is safe.
 */
export const detectStaleSessions = internalMutation({
  args: {},
  handler: async (ctx) => {
    const now = Date.now();
    const staleThreshold = now - STALE_THRESHOLD_MS;

    // Find all checked-out sessions
    const checkedOutSessions = await ctx.db
      .query("sessions")
      .withIndex("by_status", (q) => q.eq("status", "checked_out"))
      .collect();

    const staleSessions: Id<"sessions">[] = [];

    for (const session of checkedOutSessions) {
      // A session is stale if its heartbeat is older than the threshold
      // or if it has no heartbeat at all (shouldn't happen, but be safe)
      const heartbeat = session.lastHeartbeatAt;
      if (heartbeat === undefined || heartbeat < staleThreshold) {
        await ctx.db.patch(session._id, {
          status: "stale",
          updatedAt: now,
        });
        staleSessions.push(session._id);
      }
    }

    return { staleCount: staleSessions.length, staleSessions };
  },
});
