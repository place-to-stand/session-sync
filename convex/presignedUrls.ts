"use node";

import { v } from "convex/values";
import { action } from "./_generated/server";
import { api } from "./_generated/api";
import {
  S3Client,
  PutObjectCommand,
  GetObjectCommand,
  CreateMultipartUploadCommand,
  UploadPartCommand,
  CompleteMultipartUploadCommand,
} from "@aws-sdk/client-s3";
import { getSignedUrl } from "@aws-sdk/s3-request-presigner";

/** Presigned URL expiry: 1 hour (in seconds). */
const PRESIGN_EXPIRY_SECONDS = 3600;

/**
 * Validate that an R2 key matches one of the allowed patterns.
 * Prevents clients from requesting presigned URLs for arbitrary bucket paths.
 *
 * Allowed formats:
 *  - _objects/{blake3hash}         (content-addressed, 64 hex chars)
 *  - _objects/{blake3hash}.ext     (content-addressed with extension)
 *  - _versions/{uuid}/v{N}/manifest.json  (version manifests)
 */
const R2_KEY_PATTERNS = [
  /^_objects\/[a-f0-9]{64}(\.\w+)?$/,
  /^_versions\/[a-f0-9-]{36}\/v\d+\/manifest\.json$/,
];

function validateR2Key(r2Key: string): void {
  const isValid = R2_KEY_PATTERNS.some((pattern) => pattern.test(r2Key));
  if (!isValid) {
    throw new Error(
      `Invalid R2 key format: "${r2Key}". ` +
        `Keys must match _objects/{blake3hash}[.ext] or _versions/{uuid}/v{N}/manifest.json`
    );
  }
}

/**
 * Build an S3-compatible client pointed at the Cloudflare R2 endpoint.
 * Credentials are stored as Convex environment variables (never on client machines).
 */
function getR2Client(): S3Client {
  const accountId = process.env.R2_ACCOUNT_ID;
  const accessKeyId = process.env.R2_ACCESS_KEY_ID;
  const secretAccessKey = process.env.R2_SECRET_ACCESS_KEY;

  if (!accountId || !accessKeyId || !secretAccessKey) {
    throw new Error(
      "R2 credentials not configured. Set R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, " +
        "and R2_SECRET_ACCESS_KEY as Convex environment variables."
    );
  }

  return new S3Client({
    region: "auto",
    endpoint: `https://${accountId}.r2.cloudflarestorage.com`,
    credentials: {
      accessKeyId,
      secretAccessKey,
    },
  });
}

function getBucketName(): string {
  const bucket = process.env.R2_BUCKET_NAME;
  if (!bucket) {
    throw new Error(
      "R2_BUCKET_NAME not configured. Set it as a Convex environment variable."
    );
  }
  return bucket;
}

/**
 * Request a presigned upload URL for a specific R2 object.
 *
 * Only the machine that holds the checkout for the session can upload.
 * This enforces that unowned sessions cannot be modified.
 */
export const requestUploadUrl = action({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    r2Key: v.string(), // Full R2 object key, e.g. "_objects/{blake3hash}.wav"
    contentType: v.optional(v.string()),
    contentLength: v.optional(v.number()),
  },
  handler: async (ctx, args) => {
    validateR2Key(args.r2Key);

    // Validate the machine holds the checkout for this session
    const session = await ctx.runQuery(api.sessions.getSession, {
      sessionId: args.sessionId,
    });

    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    if (session.checkedOutBy !== args.machineId) {
      throw new Error(
        `Upload denied: session "${session.name}" is not checked out by this machine. ` +
          `Only the checkout holder can upload files.`
      );
    }

    // Verify the machine exists
    const machine = await ctx.runQuery(api.machines.getMachine, {
      machineId: args.machineId,
    });
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }

    // Generate presigned PUT URL
    const client = getR2Client();
    const bucket = getBucketName();

    const command = new PutObjectCommand({
      Bucket: bucket,
      Key: args.r2Key,
      ...(args.contentType && { ContentType: args.contentType }),
      ...(args.contentLength && { ContentLength: args.contentLength }),
    });

    const url = await getSignedUrl(client, command, {
      expiresIn: PRESIGN_EXPIRY_SECONDS,
    });

    return {
      url,
      method: "PUT" as const,
      expiresIn: PRESIGN_EXPIRY_SECONDS,
      r2Key: args.r2Key,
    };
  },
});

/**
 * Request a presigned download URL for a specific R2 object.
 *
 * Any registered machine can download -- no checkout ownership required.
 * This enables spectator pulls and "Pull Released Version".
 */
export const requestDownloadUrl = action({
  args: {
    machineId: v.id("machines"),
    r2Key: v.string(), // Full R2 object key
  },
  handler: async (ctx, args) => {
    validateR2Key(args.r2Key);

    // Verify the machine is registered (any registered machine can download)
    const machine = await ctx.runQuery(api.machines.getMachine, {
      machineId: args.machineId,
    });
    if (!machine) {
      throw new Error(
        `Machine ${args.machineId} not found. Register the machine first.`
      );
    }

    const client = getR2Client();
    const bucket = getBucketName();

    const command = new GetObjectCommand({
      Bucket: bucket,
      Key: args.r2Key,
    });

    const url = await getSignedUrl(client, command, {
      expiresIn: PRESIGN_EXPIRY_SECONDS,
    });

    return {
      url,
      method: "GET" as const,
      expiresIn: PRESIGN_EXPIRY_SECONDS,
      r2Key: args.r2Key,
    };
  },
});

/**
 * Request presigned URLs for a multipart upload. Used for files >= 100 MiB.
 *
 * Returns all part URLs + the multipart upload ID in one round trip so the
 * Rust client doesn't need N separate Convex calls.
 *
 * Flow:
 * 1. This action creates the multipart upload on R2
 * 2. Returns presigned URLs for each part
 * 3. Client uploads parts directly to R2 using the presigned URLs
 * 4. Client calls completeMultipartUpload (separate action) to finalize
 */
export const requestMultipartPresignedUrls = action({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    r2Key: v.string(),
    numParts: v.number(),
    contentType: v.optional(v.string()),
  },
  handler: async (ctx, args) => {
    validateR2Key(args.r2Key);

    if (args.numParts < 1 || args.numParts > 10000) {
      throw new Error(
        `numParts must be between 1 and 10000 (got ${args.numParts})`
      );
    }

    // Validate checkout ownership
    const session = await ctx.runQuery(api.sessions.getSession, {
      sessionId: args.sessionId,
    });

    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    if (session.checkedOutBy !== args.machineId) {
      throw new Error(
        `Upload denied: session "${session.name}" is not checked out by this machine.`
      );
    }

    const machine = await ctx.runQuery(api.machines.getMachine, {
      machineId: args.machineId,
    });
    if (!machine) {
      throw new Error(`Machine ${args.machineId} not found`);
    }

    const client = getR2Client();
    const bucket = getBucketName();

    // Step 1: Create the multipart upload
    const createCommand = new CreateMultipartUploadCommand({
      Bucket: bucket,
      Key: args.r2Key,
      ...(args.contentType && { ContentType: args.contentType }),
    });

    const createResponse = await client.send(createCommand);
    const uploadId = createResponse.UploadId;

    if (!uploadId) {
      throw new Error(
        "Failed to create multipart upload -- no UploadId returned."
      );
    }

    // Step 2: Generate presigned URLs for each part
    const partUrls: Array<{ partNumber: number; url: string }> = [];

    for (let partNumber = 1; partNumber <= args.numParts; partNumber++) {
      const uploadPartCommand = new UploadPartCommand({
        Bucket: bucket,
        Key: args.r2Key,
        UploadId: uploadId,
        PartNumber: partNumber,
      });

      const url = await getSignedUrl(client, uploadPartCommand, {
        expiresIn: PRESIGN_EXPIRY_SECONDS,
      });

      partUrls.push({ partNumber, url });
    }

    return {
      uploadId,
      r2Key: args.r2Key,
      partUrls,
      expiresIn: PRESIGN_EXPIRY_SECONDS,
    };
  },
});

/**
 * Complete a multipart upload after all parts have been uploaded.
 *
 * The client provides the ETags returned by R2 for each part upload.
 */
export const completeMultipartUpload = action({
  args: {
    sessionId: v.id("sessions"),
    machineId: v.id("machines"),
    r2Key: v.string(),
    uploadId: v.string(),
    parts: v.array(
      v.object({
        partNumber: v.number(),
        etag: v.string(),
      })
    ),
  },
  handler: async (ctx, args) => {
    validateR2Key(args.r2Key);

    // Validate checkout ownership
    const session = await ctx.runQuery(api.sessions.getSession, {
      sessionId: args.sessionId,
    });

    if (!session) {
      throw new Error(`Session ${args.sessionId} not found`);
    }

    if (session.checkedOutBy !== args.machineId) {
      throw new Error(
        `Upload denied: session "${session.name}" is not checked out by this machine.`
      );
    }

    const client = getR2Client();
    const bucket = getBucketName();

    const completeCommand = new CompleteMultipartUploadCommand({
      Bucket: bucket,
      Key: args.r2Key,
      UploadId: args.uploadId,
      MultipartUpload: {
        Parts: args.parts.map((p) => ({
          PartNumber: p.partNumber,
          ETag: p.etag,
        })),
      },
    });

    const response = await client.send(completeCommand);

    return {
      r2Key: args.r2Key,
      location: response.Location,
      etag: response.ETag,
    };
  },
});

/**
 * Request presigned download URLs in batch. Useful for pulling a full
 * version (many files at once).
 */
export const requestBatchDownloadUrls = action({
  args: {
    machineId: v.id("machines"),
    r2Keys: v.array(v.string()),
  },
  handler: async (ctx, args) => {
    if (args.r2Keys.length === 0) {
      return { urls: [] };
    }

    if (args.r2Keys.length > 1000) {
      throw new Error(
        `Too many keys in batch request (${args.r2Keys.length}). Maximum is 1000.`
      );
    }

    // Validate all keys before proceeding
    for (const key of args.r2Keys) {
      validateR2Key(key);
    }

    // Verify the machine is registered
    const machine = await ctx.runQuery(api.machines.getMachine, {
      machineId: args.machineId,
    });
    if (!machine) {
      throw new Error(
        `Machine ${args.machineId} not found. Register the machine first.`
      );
    }

    const client = getR2Client();
    const bucket = getBucketName();

    const urls = await Promise.all(
      args.r2Keys.map(async (r2Key) => {
        const command = new GetObjectCommand({
          Bucket: bucket,
          Key: r2Key,
        });

        const url = await getSignedUrl(client, command, {
          expiresIn: PRESIGN_EXPIRY_SECONDS,
        });

        return { r2Key, url };
      })
    );

    return {
      urls,
      expiresIn: PRESIGN_EXPIRY_SECONDS,
    };
  },
});
