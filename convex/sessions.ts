import { v } from "convex/values";
import { mutation, query } from "./_generated/server";

/**
 * List all non-archived sessions (or optionally include archived).
 */
export const listSessions = query({
  args: {
    includeArchived: v.optional(v.boolean()),
  },
  handler: async (ctx, args) => {
    if (args.includeArchived) {
      return await ctx.db.query("sessions").collect();
    }
    // Return sessions that are NOT archived. We query all and filter because
    // Convex index queries don't support "not equal" natively.
    const sessions = await ctx.db.query("sessions").collect();
    return sessions.filter((s) => s.status !== "archived");
  },
});

/**
 * Get a single session by ID.
 */
export const getSession = query({
  args: {
    sessionId: v.id("sessions"),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }
    return session;
  },
});

/**
 * Atomic conditional checkout: SET checkedOutBy=me WHERE checkedOutBy IS NULL.
 *
 * This is the compare-and-swap that prevents two engineers from checking out
 * the same session simultaneously. Convex mutations are serialized, so this
 * is inherently atomic.
 */
export const checkoutSession = mutation({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    // Verify the machine exists
    const machine = await ctx.db.get(args.machineId);
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }

    // Archived sessions cannot be checked out
    if (session.status === "archived") {
      throw new Error(
        `Session "${session.name}" is archived. Unarchive it before checking out.`
      );
    }

    // Idempotent: if this machine already holds the checkout, return success
    if (session.checkedOutBy === args.machineId) {
      return { success: true, checkedOutAt: session.checkedOutAt! };
    }

    // Atomic compare-and-swap: only check out if nobody else has it
    if (session.checkedOutBy !== undefined) {
      // Someone else has it checked out — find out who for a useful error
      const holder = await ctx.db.get(session.checkedOutBy);
      const holderName = holder?.displayName ?? "unknown machine";
      throw new Error(
        `Session "${session.name}" is already checked out by ${holderName}. ` +
        `Wait for them to release, or claim if the session is stale.`
      );
    }

    const now = Date.now();
    await ctx.db.patch(args.sessionId, {
      checkedOutBy: args.machineId,
      checkedOutAt: now,
      lastHeartbeatAt: now,
      status: "checked_out",
      updatedAt: now,
    });

    return { success: true, checkedOutAt: now };
  },
});

/**
 * Release a checked-out session. Only the machine that holds the checkout
 * can release it.
 */
export const releaseSession = mutation({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    if (session.checkedOutBy === undefined) {
      throw new Error(
        `Session "${session.name}" is not checked out — nothing to release.`
      );
    }

    // Only the holder can release
    if (session.checkedOutBy !== args.machineId) {
      const holder = await ctx.db.get(session.checkedOutBy);
      const holderName = holder?.displayName ?? "unknown machine";
      throw new Error(
        `Session "${session.name}" is checked out by ${holderName}, ` +
        `not by the requesting machine. Only the checkout holder can release.`
      );
    }

    const now = Date.now();
    await ctx.db.patch(args.sessionId, {
      checkedOutBy: undefined,
      checkedOutAt: undefined,
      lastHeartbeatAt: undefined,
      status: "available",
      updatedAt: now,
    });

    return { success: true, releasedAt: now };
  },
});

/**
 * Claim a stale session. Only allowed when status is "stale", meaning the
 * machine holding checkout has missed heartbeats for 30+ minutes.
 */
export const claimSession = mutation({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    const machine = await ctx.db.get(args.machineId);
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }

    if (session.status !== "stale") {
      throw new Error(
        `Session "${session.name}" is not stale (current status: ${session.status}). ` +
        `You can only claim a session with stale checkout.`
      );
    }

    const now = Date.now();
    await ctx.db.patch(args.sessionId, {
      checkedOutBy: args.machineId,
      checkedOutAt: now,
      lastHeartbeatAt: now,
      status: "checked_out",
      updatedAt: now,
    });

    return { success: true, claimedAt: now };
  },
});

/**
 * Archive a session. Stops syncing, but data remains in R2.
 * Session must be available (not checked out) to archive.
 */
export const archiveSession = mutation({
  args: {
    sessionId: v.id("sessions"),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    if (session.status === "archived") {
      throw new Error(`Session "${session.name}" is already archived.`);
    }

    if (session.status === "checked_out" || session.status === "stale") {
      throw new Error(
        `Session "${session.name}" is currently checked out (status: ${session.status}). ` +
        `Release or claim it before archiving.`
      );
    }

    const now = Date.now();
    await ctx.db.patch(args.sessionId, {
      status: "archived",
      updatedAt: now,
    });

    return { success: true, archivedAt: now };
  },
});

/**
 * Unarchive a session, returning it to "available" status.
 */
export const unarchiveSession = mutation({
  args: {
    sessionId: v.id("sessions"),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    if (session.status !== "archived") {
      throw new Error(
        `Session "${session.name}" is not archived (current status: ${session.status}).`
      );
    }

    const now = Date.now();
    await ctx.db.patch(args.sessionId, {
      status: "available",
      updatedAt: now,
    });

    return { success: true, unarchivedAt: now };
  },
});

/**
 * Create a new session. Generates a UUID-based R2 prefix so that
 * session renames only change the Convex display name.
 */
export const createSession = mutation({
  args: {
    name: v.string(),
    r2Prefix: v.string(), // Caller generates "sessions/{uuid}"
  },
  handler: async (ctx, args) => {
    if (!args.name.trim()) {
      throw new Error("Session name cannot be empty.");
    }
    if (!args.r2Prefix.trim()) {
      throw new Error("R2 prefix cannot be empty.");
    }

    const now = Date.now();
    const sessionId = await ctx.db.insert("sessions", {
      name: args.name.trim(),
      r2Prefix: args.r2Prefix,
      status: "available",
      createdAt: now,
      updatedAt: now,
    });

    return sessionId;
  },
});

/**
 * Rename a session (display name only — R2 prefix is immutable).
 */
export const renameSession = mutation({
  args: {
    sessionId: v.id("sessions"),
    name: v.string(),
  },
  handler: async (ctx, args) => {
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    if (!args.name.trim()) {
      throw new Error("Session name cannot be empty.");
    }

    await ctx.db.patch(args.sessionId, {
      name: args.name.trim(),
      updatedAt: Date.now(),
    });

    return { success: true };
  },
});
