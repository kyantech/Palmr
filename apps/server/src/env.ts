import { z } from "zod";

const envSchema = z.object({
  S3_ENDPOINT: z.string().min(1, "S3_ENDPOINT is required"),
  S3_PORT: z.string().optional(),
  S3_USE_SSL: z.string().optional(),
  S3_ACCESS_KEY: z.string().min(1, "S3_ACCESS_KEY is required"),
  S3_SECRET_KEY: z.string().min(1, "S3_SECRET_KEY is required"),
  S3_REGION: z.string().min(1, "S3_REGION is required"),
  S3_BUCKET_NAME: z.string().min(1, "S3_BUCKET_NAME is required"),
  S3_FORCE_PATH_STYLE: z.union([z.literal("true"), z.literal("false")]).default("false"),
  S3_REJECT_UNAUTHORIZED: z.union([z.literal("true"), z.literal("false")]).default("true"),
  PRESIGNED_URL_EXPIRATION: z.string().optional().default("3600"),
  SECURE_SITE: z.union([z.literal("true"), z.literal("false")]).default("false"),
  DATABASE_URL: z.string().optional().default("file:/app/server/prisma/palmr.db"),
  CUSTOM_PATH: z.string().optional(),
});

export const env = envSchema.parse(process.env);
