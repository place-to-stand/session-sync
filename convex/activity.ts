import { v } from "convex/values";
import { mutation, query } from "./_generated/server";
import { Doc, Id } from "./_generated/dataModel";

/**
 * Valid activity actions. Matches the schema union type.
 */
const activityAction = v.union(
  v.literal("checkout"),
  v.literal("release"),
  v.literal("pull"),
  v.literal("push"),
  v.literal("archive"),
  v.literal("unarchive"),
  v.literal("request"),
  v.literal("claim"),
);

/**
 * Log an activity event for a session. Called by other mutations/actions
 * or directly by the Rust client for client-side events (pull, push).
 */
export const logActivity = mutation({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    action: activityAction,
    details: v.optional(v.string()),
  },
  handler: async (ctx, args) => {
    // Verify the session exists
    const session = await ctx.db.get(args.sessionId);
    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    // Verify the machine exists
    const machine = await ctx.db.get(args.machineId);
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }

    const now = Date.now();
    const activityId = await ctx.db.insert("activity", {
      sessionId: args.sessionId,
      machineId: args.machineId,
      action: args.action,
      details: args.details,
      createdAt: now,
    });

    return activityId;
  },
});

/**
 * List activity for a specific session, ordered by most recent first.
 * Supports pagination via limit + optional cursor-based offset.
 */
export const listActivity = query({
  args: {
    sessionId: v.id("sessions"),
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 50;

    const activities = await ctx.db
      .query("activity")
      .withIndex("by_session_created", (q) =>
        q.eq("sessionId", args.sessionId)
      )
      .order("desc")
      .take(limit);

    // Enrich with machine display names for the frontend
    const enriched = await Promise.all(
      activities.map(async (act) => {
        const machine = await ctx.db.get(act.machineId);
        return {
          ...act,
          machineDisplayName: machine?.displayName ?? "Unknown machine",
          machineMachineId: machine?.machineId ?? "unknown",
        };
      })
    );

    return enriched;
  },
});

/**
 * List all activity across all sessions, ordered by most recent first.
 * Useful for the global activity feed in the admin/overview panel.
 */
export const listAllActivity = query({
  args: {
    limit: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    const limit = args.limit ?? 100;

    const activities = await ctx.db
      .query("activity")
      .withIndex("by_createdAt")
      .order("desc")
      .take(limit);

    // Enrich with machine and session names for the frontend
    const enriched = await Promise.all(
      activities.map(async (act) => {
        const machine = await ctx.db.get(act.machineId);
        const session = await ctx.db.get(act.sessionId);
        return {
          ...act,
          machineDisplayName: machine?.displayName ?? "Unknown machine",
          machineMachineId: machine?.machineId ?? "unknown",
          sessionName: session?.name ?? "Unknown session",
        };
      })
    );

    return enriched;
  },
});
