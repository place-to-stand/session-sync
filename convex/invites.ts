"use node";

import { v } from "convex/values";
import { action } from "./_generated/server";
import { api } from "./_generated/api";
import { Id } from "./_generated/dataModel";
import crypto from "crypto";

/** Invite link expiry: 7 days in milliseconds. */
const INVITE_EXPIRY_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * Get the HMAC signing secret from environment variables.
 */
function getSigningSecret(): string {
  const secret = process.env.INVITE_SIGNING_SECRET;
  if (!secret) {
    throw new Error(
      "INVITE_SIGNING_SECRET not configured. Set it as a Convex environment variable. " +
      "Generate one with: openssl rand -hex 32"
    );
  }
  return secret;
}

/**
 * Create an HMAC-SHA256 signature for an invite payload.
 */
function signPayload(payload: string, secret: string): string {
  return crypto
    .createHmac("sha256", secret)
    .update(payload)
    .digest("hex");
}

/**
 * Verify an HMAC-SHA256 signature using constant-time comparison.
 */
function verifySignature(
  payload: string,
  signature: string,
  secret: string
): boolean {
  const expected = signPayload(payload, secret);
  // Constant-time comparison to prevent timing attacks
  return crypto.timingSafeEqual(
    Buffer.from(signature, "hex"),
    Buffer.from(expected, "hex")
  );
}

/**
 * Create a signed invite link. Only admins should call this.
 *
 * The invite token contains:
 * - A random nonce (for uniqueness)
 * - The invitee's email and name
 * - An expiry timestamp
 * - An HMAC-SHA256 signature over all of the above
 *
 * The token is strictly single-use: redeemInvite marks it as redeemed.
 */
export const createInvite = action({
  args: {
    createdBy: v.id("users"),
    email: v.string(),
    name: v.string(),
    role: v.union(v.literal("admin"), v.literal("engineer")),
  },
  handler: async (ctx, args) => {
    if (!args.email.trim()) {
      throw new Error("Email cannot be empty.");
    }
    if (!args.name.trim()) {
      throw new Error("Name cannot be empty.");
    }

    // Verify the creator exists and is an admin
    const creator = await ctx.runQuery(api.users.getUser, {
      userId: args.createdBy,
    });
    if (!creator) {
      throw new Error(`User ${args.createdBy} not found`);
    }
    if (creator.role !== "admin") {
      throw new Error("Only admins can create invite links.");
    }

    const secret = getSigningSecret();
    const nonce = crypto.randomBytes(16).toString("hex");
    const now = Date.now();
    const expiresAt = now + INVITE_EXPIRY_MS;

    // The payload that gets signed — this binds the token to a specific
    // invitee, expiry, and nonce so it can't be tampered with.
    const payload = JSON.stringify({
      nonce,
      email: args.email.trim().toLowerCase(),
      name: args.name.trim(),
      role: args.role,
      expiresAt,
    });

    const signature = signPayload(payload, secret);

    // The token is the base64-encoded payload + "." + the signature
    const token =
      Buffer.from(payload).toString("base64url") + "." + signature;

    // Store the invite in Convex for single-use enforcement
    const inviteId: Id<"invites"> = await ctx.runMutation(
      api.inviteHelpers._storeInvite,
      {
        createdBy: args.createdBy,
        email: args.email.trim().toLowerCase(),
        name: args.name.trim(),
        role: args.role,
        token,
        expiresAt,
        createdAt: now,
      }
    );

    return {
      inviteId,
      token,
      expiresAt,
    };
  },
});

/**
 * Redeem an invite token. Validates the HMAC signature, expiry, and
 * single-use constraint. On success, creates user + machine records
 * and returns auth info.
 */
export const redeemInvite = action({
  args: {
    token: v.string(),
    machineId: v.string(), // Stable local machine ID (e.g. "nyc-macbook")
    machineDisplayName: v.string(),
    appVersion: v.string(),
    platform: v.string(),
  },
  handler: async (ctx, args) => {
    if (!args.token.trim()) {
      throw new Error("Invite token cannot be empty.");
    }

    const secret = getSigningSecret();

    // Parse the token: base64url(payload) + "." + signature
    const parts = args.token.split(".");
    if (parts.length !== 2) {
      throw new Error("Invalid invite token format.");
    }

    const [encodedPayload, signature] = parts;

    let payload: string;
    try {
      payload = Buffer.from(encodedPayload, "base64url").toString("utf-8");
    } catch {
      throw new Error("Invalid invite token: malformed encoding.");
    }

    // Verify HMAC signature (constant-time)
    let isValid: boolean;
    try {
      isValid = verifySignature(payload, signature, secret);
    } catch {
      throw new Error("Invalid invite token: signature verification failed.");
    }

    if (!isValid) {
      throw new Error(
        "Invalid invite token: signature does not match. " +
        "This token may have been tampered with."
      );
    }

    // Parse the payload
    let tokenData: {
      nonce: string;
      email: string;
      name: string;
      role: "admin" | "engineer";
      expiresAt: number;
    };
    try {
      tokenData = JSON.parse(payload);
    } catch {
      throw new Error("Invalid invite token: malformed payload.");
    }

    // Check expiry
    if (Date.now() > tokenData.expiresAt) {
      throw new Error(
        "This invite link has expired. Ask the admin to send a new one."
      );
    }

    // Look up the invite in the database for single-use enforcement
    const invite = await ctx.runQuery(api.inviteHelpers._getInviteByToken, {
      token: args.token,
    });

    if (!invite) {
      throw new Error(
        "Invite not found in database. It may have been revoked."
      );
    }

    if (invite.redeemedAt) {
      throw new Error(
        "This invite link has already been used. Each invite is single-use. " +
        "Ask the admin to send a new one."
      );
    }

    if (invite.revokedAt) {
      throw new Error(
        "This invite link has been revoked by an admin."
      );
    }

    // All validations passed — create user + machine records
    const result = await ctx.runMutation(api.inviteHelpers._redeemInvite, {
      inviteId: invite._id,
      email: tokenData.email,
      name: tokenData.name,
      role: tokenData.role,
      machineId: args.machineId,
      machineDisplayName: args.machineDisplayName,
      appVersion: args.appVersion,
      platform: args.platform,
    });

    return result;
  },
});
