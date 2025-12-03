import { z } from "zod";

const envSchema = z.object({
  // Storage configuration
  ENABLE_S3: z.union([z.literal("true"), z.literal("false")]).default("false"),
  S3_ENDPOINT: z.string().optional(),
  S3_PORT: z.string().optional(),
  S3_USE_SSL: z.string().optional(),
  S3_ACCESS_KEY: z.string().optional(),
  S3_SECRET_KEY: z.string().optional(),
  S3_REGION: z.string().optional(),
  S3_BUCKET_NAME: z.string().optional(),
  S3_FORCE_PATH_STYLE: z.union([z.literal("true"), z.literal("false")]).default("false"),
  S3_REJECT_UNAUTHORIZED: z.union([z.literal("true"), z.literal("false")]).default("true"),

  // Legacy encryption vars (kept for backward compatibility but not used with S3/Garage)
  ENCRYPTION_KEY: z.string().optional(),
  DISABLE_FILESYSTEM_ENCRYPTION: z.union([z.literal("true"), z.literal("false")]).default("true"),

  // Application configuration
  PRESIGNED_URL_EXPIRATION: z.string().optional().default("3600"),
  SECURE_SITE: z.union([z.literal("true"), z.literal("false")]).default("false"),
  STORAGE_URL: z.string().optional(), // Storage URL for internal storage presigned URLs (required when ENABLE_S3=false, e.g., https://syrg.palmr.com or http://192.168.1.100:9379)
  DATABASE_URL: z.string().optional().default("file:/app/server/prisma/palmr.db"),
  CUSTOM_PATH: z.string().optional(),
});

export const env = envSchema.parse(process.env);
