import { v } from "convex/values";
import { mutation, query, internalMutation, internalQuery } from "./_generated/server";
import { Doc, Id } from "./_generated/dataModel";

/**
 * Internal: store an invite record in the database.
 * Called by the createInvite action.
 */
export const _storeInvite = mutation({
  args: {
    createdBy: v.id("users"),
    email: v.string(),
    name: v.string(),
    role: v.union(v.literal("admin"), v.literal("engineer")),
    token: v.string(),
    expiresAt: v.number(),
    createdAt: v.number(),
  },
  handler: async (ctx, args) => {
    const inviteId = await ctx.db.insert("invites", {
      createdBy: args.createdBy,
      email: args.email,
      name: args.name,
      role: args.role,
      token: args.token,
      expiresAt: args.expiresAt,
      createdAt: args.createdAt,
    });
    return inviteId;
  },
});

/**
 * Internal: look up an invite by its token string.
 * Called by the redeemInvite action for single-use validation.
 */
export const _getInviteByToken = query({
  args: {
    token: v.string(),
  },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("invites")
      .withIndex("by_token", (q) => q.eq("token", args.token))
      .unique();
  },
});

/**
 * Internal: redeem an invite — mark it as used, create user + machine records.
 * Called by the redeemInvite action after all validations pass.
 */
export const _redeemInvite = mutation({
  args: {
    inviteId: v.id("invites"),
    email: v.string(),
    name: v.string(),
    role: v.union(v.literal("admin"), v.literal("engineer")),
    machineId: v.string(),
    machineDisplayName: v.string(),
    appVersion: v.string(),
    platform: v.string(),
  },
  handler: async (ctx, args) => {
    const now = Date.now();

    // Double-check single-use (in case of race between action read and this mutation)
    const invite = await ctx.db.get(args.inviteId);
    if (!invite) {
      throw new Error("Invite not found.");
    }
    if (invite.redeemedAt) {
      throw new Error("This invite has already been redeemed.");
    }
    if (invite.revokedAt) {
      throw new Error("This invite has been revoked.");
    }

    // Check if user already exists with this email
    let userId: Id<"users">;
    const existingUser = await ctx.db
      .query("users")
      .withIndex("by_email", (q) => q.eq("email", args.email))
      .unique();

    if (existingUser) {
      userId = existingUser._id;
    } else {
      // Create the new user
      userId = await ctx.db.insert("users", {
        name: args.name,
        email: args.email,
        role: args.role,
        createdAt: now,
      });
    }

    // Check if machine already exists
    let convexMachineId: Id<"machines">;
    const existingMachine = await ctx.db
      .query("machines")
      .withIndex("by_machineId", (q) => q.eq("machineId", args.machineId))
      .unique();

    if (existingMachine) {
      // Update existing machine to point to this user
      await ctx.db.patch(existingMachine._id, {
        userId,
        displayName: args.machineDisplayName,
        appVersion: args.appVersion,
        platform: args.platform,
        lastHeartbeatAt: now,
      });
      convexMachineId = existingMachine._id;
    } else {
      // Create new machine record
      convexMachineId = await ctx.db.insert("machines", {
        userId,
        machineId: args.machineId,
        displayName: args.machineDisplayName,
        lastHeartbeatAt: now,
        appVersion: args.appVersion,
        platform: args.platform,
        createdAt: now,
      });
    }

    // Mark invite as redeemed (single-use enforcement)
    await ctx.db.patch(args.inviteId, {
      redeemedAt: now,
      redeemedBy: userId,
    });

    return {
      userId,
      machineId: convexMachineId,
      email: args.email,
      name: args.name,
      role: args.role,
    };
  },
});

/**
 * Revoke an invite (admin action). Prevents future redemption.
 */
export const revokeInvite = mutation({
  args: {
    inviteId: v.id("invites"),
  },
  handler: async (ctx, args) => {
    const invite = await ctx.db.get(args.inviteId);
    if (!invite) {
      throw new Error(`Invite ${args.inviteId} not found`);
    }

    if (invite.redeemedAt) {
      throw new Error("Cannot revoke an already-redeemed invite.");
    }

    if (invite.revokedAt) {
      throw new Error("This invite is already revoked.");
    }

    await ctx.db.patch(args.inviteId, {
      revokedAt: Date.now(),
    });

    return { success: true };
  },
});

/**
 * List all invites, ordered by creation time descending.
 */
export const listInvites = query({
  args: {},
  handler: async (ctx) => {
    return await ctx.db.query("invites").order("desc").collect();
  },
});
