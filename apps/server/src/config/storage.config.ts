import * as fs from "fs";
import process from "node:process";
import { S3Client } from "@aws-sdk/client-s3";

import { env } from "../env";
import { StorageConfig } from "../types/storage";

/**
 * Load internal storage credentials if they exist
 * This provides S3-compatible storage automatically when ENABLE_S3=false
 */
function loadInternalStorageCredentials(): Partial<StorageConfig> | null {
  const credentialsPath = "/app/server/.minio-credentials";

  try {
    if (fs.existsSync(credentialsPath)) {
      const content = fs.readFileSync(credentialsPath, "utf-8");
      const credentials: any = {};

      content.split("\n").forEach((line) => {
        const [key, value] = line.split("=");
        if (key && value) {
          credentials[key.trim()] = value.trim();
        }
      });

      console.log("[STORAGE] Using internal storage system");

      return {
        endpoint: credentials.S3_ENDPOINT || "127.0.0.1",
        port: parseInt(credentials.S3_PORT || "9379", 10),
        useSSL: credentials.S3_USE_SSL === "true",
        accessKey: credentials.S3_ACCESS_KEY,
        secretKey: credentials.S3_SECRET_KEY,
        region: credentials.S3_REGION || "default",
        bucketName: credentials.S3_BUCKET_NAME || "palmr-files",
        forcePathStyle: true,
      };
    }
  } catch (error) {
    console.warn("[STORAGE] Could not load internal storage credentials:", error);
  }

  return null;
}

/**
 * Storage configuration:
 * - Default (ENABLE_S3=false or not set): Internal storage (auto-configured, zero config)
 * - ENABLE_S3=true: External S3 (AWS, S3-compatible, etc) using env vars
 */
const internalStorageConfig = env.ENABLE_S3 === "true" ? null : loadInternalStorageCredentials();

export const storageConfig: StorageConfig = (internalStorageConfig as StorageConfig) || {
  endpoint: env.S3_ENDPOINT || "",
  port: env.S3_PORT ? Number(env.S3_PORT) : undefined,
  useSSL: env.S3_USE_SSL === "true",
  accessKey: env.S3_ACCESS_KEY || "",
  secretKey: env.S3_SECRET_KEY || "",
  region: env.S3_REGION || "",
  bucketName: env.S3_BUCKET_NAME || "",
  forcePathStyle: env.S3_FORCE_PATH_STYLE === "true",
};

if (storageConfig.useSSL && env.S3_REJECT_UNAUTHORIZED === "false") {
  const originalRejectUnauthorized = process.env.NODE_TLS_REJECT_UNAUTHORIZED;
  if (!originalRejectUnauthorized) {
    process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";
    (global as any).PALMR_ORIGINAL_TLS_SETTING = originalRejectUnauthorized;
  }
}

/**
 * Storage is ALWAYS S3-compatible:
 * - ENABLE_S3=false → Internal storage (automatic)
 * - ENABLE_S3=true  → External S3 (AWS, S3-compatible, etc)
 */
const hasValidConfig = storageConfig.endpoint && storageConfig.accessKey && storageConfig.secretKey;

export const s3Client = hasValidConfig
  ? new S3Client({
      endpoint: storageConfig.useSSL
        ? `https://${storageConfig.endpoint}${storageConfig.port ? `:${storageConfig.port}` : ""}`
        : `http://${storageConfig.endpoint}${storageConfig.port ? `:${storageConfig.port}` : ""}`,
      region: storageConfig.region,
      credentials: {
        accessKeyId: storageConfig.accessKey,
        secretAccessKey: storageConfig.secretKey,
      },
      forcePathStyle: storageConfig.forcePathStyle,
    })
  : null;

export const bucketName = storageConfig.bucketName;

/**
 * Storage is always S3-compatible
 * ENABLE_S3=true means EXTERNAL S3, otherwise uses internal storage
 */
export const isS3Enabled = s3Client !== null;
export const isExternalS3 = env.ENABLE_S3 === "true";
export const isInternalStorage = s3Client !== null && env.ENABLE_S3 !== "true";

/**
 * Creates a public S3 client for presigned URL generation.
 * - Internal storage (ENABLE_S3=false): Uses STORAGE_URL (e.g., https://syrg.palmr.com)
 * - External S3 (ENABLE_S3=true): Uses the original S3 endpoint configuration
 *
 * @returns S3Client configured with public endpoint, or null if S3 is disabled
 */
export function createPublicS3Client(): S3Client | null {
  if (!s3Client) {
    return null;
  }

  let publicEndpoint: string;

  if (isInternalStorage) {
    // Internal storage: use STORAGE_URL
    if (!env.STORAGE_URL) {
      throw new Error(
        "[STORAGE] STORAGE_URL environment variable is required when using internal storage (ENABLE_S3=false). " +
          "Set STORAGE_URL to your public storage URL with protocol (e.g., https://syrg.palmr.com or http://192.168.1.100:9379)"
      );
    }
    publicEndpoint = env.STORAGE_URL;
  } else {
    // External S3: use the original endpoint configuration
    publicEndpoint = storageConfig.useSSL
      ? `https://${storageConfig.endpoint}${storageConfig.port ? `:${storageConfig.port}` : ""}`
      : `http://${storageConfig.endpoint}${storageConfig.port ? `:${storageConfig.port}` : ""}`;
  }

  console.log(`[STORAGE] Creating public S3 client with endpoint: ${publicEndpoint}`);

  return new S3Client({
    endpoint: publicEndpoint,
    region: storageConfig.region,
    credentials: {
      accessKeyId: storageConfig.accessKey,
      secretAccessKey: storageConfig.secretKey,
    },
    forcePathStyle: storageConfig.forcePathStyle,
  });
}
