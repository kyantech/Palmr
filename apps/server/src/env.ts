import { z } from "zod";

const envSchema = z.object({
  FRONTEND_URL: z.string().url().min(1),
  MINIO_ENDPOINT: z.string().min(1),
  MINIO_PORT: z.string().min(1),
  MINIO_USE_SSL: z.string().min(1),
  MINIO_ROOT_PASSWORD: z.string().min(1),
  MINIO_ROOT_USER: z.string().min(1),
  MINIO_REGION: z.string().min(1),
  MINIO_BUCKET_NAME: z.string().min(1),
  PORT: z.string().min(1),
  DATABASE_URL: z.string().min(1),
  SERVER_IP: z.string().min(1),
  MAX_FILESIZE: z.string().min(1),
});

export const env = envSchema.parse(process.env);
