import { v } from "convex/values";
import { mutation, query } from "./_generated/server";
import { Doc, Id } from "./_generated/dataModel";

/**
 * Get a user by their Convex document ID.
 */
export const getUser = query({
  args: {
    userId: v.id("users"),
  },
  handler: async (ctx, args) => {
    const user = await ctx.db.get(args.userId);
    if (!user) {
      throw new Error(`User ${args.userId} not found`);
    }
    return user;
  },
});

/**
 * Look up a user by email address.
 */
export const getUserByEmail = query({
  args: {
    email: v.string(),
  },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("users")
      .withIndex("by_email", (q) => q.eq("email", args.email.toLowerCase()))
      .unique();
  },
});

/**
 * List all users.
 */
export const listUsers = query({
  args: {},
  handler: async (ctx) => {
    return await ctx.db.query("users").collect();
  },
});

/**
 * Create a new user (admin bootstrap — used for the initial admin setup).
 */
export const createUser = mutation({
  args: {
    name: v.string(),
    email: v.string(),
    role: v.union(v.literal("admin"), v.literal("engineer")),
  },
  handler: async (ctx, args) => {
    if (!args.name.trim()) {
      throw new Error("Name cannot be empty.");
    }
    if (!args.email.trim()) {
      throw new Error("Email cannot be empty.");
    }

    const normalizedEmail = args.email.trim().toLowerCase();

    // Check for duplicate email
    const existing = await ctx.db
      .query("users")
      .withIndex("by_email", (q) => q.eq("email", normalizedEmail))
      .unique();

    if (existing) {
      throw new Error(
        `A user with email "${normalizedEmail}" already exists.`
      );
    }

    const userId = await ctx.db.insert("users", {
      name: args.name.trim(),
      email: normalizedEmail,
      role: args.role,
      createdAt: Date.now(),
    });

    return userId;
  },
});
