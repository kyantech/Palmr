import { S3Client } from "@aws-sdk/client-s3";

import { env } from "../env";
import { StorageConfig } from "../types/storage";

export const storageConfig: StorageConfig = {
  endpoint: env.S3_ENDPOINT || "",
  port: env.S3_PORT ? Number(env.S3_PORT) : undefined,
  useSSL: env.S3_USE_SSL === "true",
  accessKey: env.S3_ACCESS_KEY || "",
  secretKey: env.S3_SECRET_KEY || "",
  region: env.S3_REGION || "",
  bucketName: env.S3_BUCKET_NAME || "",
  forcePathStyle: env.S3_FORCE_PATH_STYLE === "true",
};

export const s3Client =
  env.ENABLE_S3 === "true"
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

export const isS3Enabled = env.ENABLE_S3 === "true";
