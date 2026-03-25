import { v } from "convex/values";
import { mutation, query } from "./_generated/server";

/**
 * Get the full configuration as an object. Reads all rows from the config
 * table and returns them as a key-value map with JSON-parsed values.
 *
 * This is the primary config query used by the frontend for real-time
 * subscription to config changes (e.g. minClientVersion).
 */
export const getConfig = query({
  args: {},
  handler: async (ctx) => {
    const configEntries = await ctx.db.query("config").collect();

    const config: Record<string, unknown> = {};
    for (const entry of configEntries) {
      try {
        config[entry.key] = JSON.parse(entry.value);
      } catch {
        // If the value is not valid JSON, return it as-is
        config[entry.key] = entry.value;
      }
    }

    return config;
  },
});

/**
 * Get a single configuration value by key.
 * Returns the parsed JSON value, or null if the key doesn't exist.
 */
export const getConfigValue = query({
  args: {
    key: v.string(),
  },
  handler: async (ctx, args) => {
    const config = await ctx.db
      .query("config")
      .withIndex("by_key", (q) => q.eq("key", args.key))
      .unique();

    if (!config) {
      return null;
    }

    try {
      return JSON.parse(config.value);
    } catch {
      return config.value;
    }
  },
});

/**
 * Convenience query: get the minimum client version required.
 *
 * Each machine reports its appVersion in the heartbeat. If a client's
 * version is below minClientVersion, the app shows "Please update
 * SessionSync" with a download link.
 */
export const getMinClientVersion = query({
  args: {},
  handler: async (ctx) => {
    const config = await ctx.db
      .query("config")
      .withIndex("by_key", (q) => q.eq("key", "minClientVersion"))
      .unique();

    if (!config) {
      return null;
    }

    try {
      return JSON.parse(config.value) as string;
    } catch {
      return config.value;
    }
  },
});

/**
 * Set a single configuration key-value pair. Creates the key if it doesn't
 * exist, updates it if it does (upsert).
 *
 * Values are stored as JSON strings for flexibility. The caller must
 * JSON-encode the value before passing it (e.g., '"0.1.0"' for a string
 * or '3600000' for a number).
 */
export const setConfig = mutation({
  args: {
    key: v.string(),
    value: v.string(), // JSON-encoded value
  },
  handler: async (ctx, args) => {
    if (!args.key.trim()) {
      throw new Error("Config key cannot be empty.");
    }

    // Validate that the value is valid JSON
    try {
      JSON.parse(args.value);
    } catch {
      throw new Error(
        `Config value must be valid JSON. Got: ${args.value}. ` +
          `Wrap strings in double quotes, e.g. '"0.1.0"'.`
      );
    }

    const existing = await ctx.db
      .query("config")
      .withIndex("by_key", (q) => q.eq("key", args.key))
      .unique();

    const now = Date.now();

    if (existing) {
      await ctx.db.patch(existing._id, {
        value: args.value,
        updatedAt: now,
      });
      return existing._id;
    }

    return await ctx.db.insert("config", {
      key: args.key.trim(),
      value: args.value,
      updatedAt: now,
    });
  },
});

/**
 * Set multiple configuration values at once. Each provided key-value pair
 * is upserted independently.
 */
export const setConfigs = mutation({
  args: {
    entries: v.array(
      v.object({
        key: v.string(),
        value: v.string(), // JSON-encoded
      })
    ),
  },
  handler: async (ctx, args) => {
    if (args.entries.length === 0) {
      throw new Error("At least one config entry must be provided.");
    }

    const now = Date.now();

    for (const entry of args.entries) {
      if (!entry.key.trim()) {
        throw new Error("Config key cannot be empty.");
      }

      // Validate JSON
      try {
        JSON.parse(entry.value);
      } catch {
        throw new Error(
          `Config value for key "${entry.key}" must be valid JSON. Got: ${entry.value}`
        );
      }

      const existing = await ctx.db
        .query("config")
        .withIndex("by_key", (q) => q.eq("key", entry.key))
        .unique();

      if (existing) {
        await ctx.db.patch(existing._id, {
          value: entry.value,
          updatedAt: now,
        });
      } else {
        await ctx.db.insert("config", {
          key: entry.key.trim(),
          value: entry.value,
          updatedAt: now,
        });
      }
    }

    return { success: true, updatedAt: now };
  },
});

/**
 * Delete a configuration key.
 */
export const deleteConfig = mutation({
  args: {
    key: v.string(),
  },
  handler: async (ctx, args) => {
    const existing = await ctx.db
      .query("config")
      .withIndex("by_key", (q) => q.eq("key", args.key))
      .unique();

    if (!existing) {
      throw new Error(`Config key "${args.key}" not found.`);
    }

    await ctx.db.delete(existing._id);
    return { success: true };
  },
});
