import * as fs from "fs/promises";
import crypto from "node:crypto";
import fastifyMultipart from "@fastify/multipart";

import { buildApp } from "./app";
import { directoriesConfig } from "./config/directories.config";
import { appRoutes } from "./modules/app/routes";
import { authProvidersRoutes } from "./modules/auth-providers/routes";
import { authRoutes } from "./modules/auth/routes";
import { fileRoutes } from "./modules/file/routes";
import { folderRoutes } from "./modules/folder/routes";
import { healthRoutes } from "./modules/health/routes";
import { inviteRoutes } from "./modules/invite/routes";
import { reverseShareRoutes } from "./modules/reverse-share/routes";
import { s3StorageRoutes } from "./modules/s3-storage/routes";
import { shareRoutes } from "./modules/share/routes";
import { storageRoutes } from "./modules/storage/routes";
import { twoFactorRoutes } from "./modules/two-factor/routes";
import { userRoutes } from "./modules/user/routes";

if (typeof globalThis.crypto === "undefined") {
  globalThis.crypto = crypto.webcrypto as any;
}

if (typeof global.crypto === "undefined") {
  (global as any).crypto = crypto.webcrypto;
}

async function ensureDirectories() {
  const dirsToCreate = [
    { path: directoriesConfig.uploads, name: "uploads" },
    { path: directoriesConfig.tempUploads, name: "temp-uploads" },
  ];

  for (const dir of dirsToCreate) {
    try {
      await fs.access(dir.path);
    } catch {
      await fs.mkdir(dir.path, { recursive: true });
      console.log(`📁 Created ${dir.name} directory: ${dir.path}`);
    }
  }
}

async function startServer() {
  const app = await buildApp();

  await ensureDirectories();
  const { isInternalStorage, isExternalS3, storageConfig } = await import("./config/storage.config.js");
  const { env } = await import("./env.js");
  const { runAutoMigration } = await import("./scripts/migrate-filesystem-to-s3.js");
  await runAutoMigration();

  await app.register(fastifyMultipart, {
    limits: {
      fieldNameSize: 100,
      fieldSize: 1024 * 1024,
      fields: 10,
      fileSize: 1024 * 1024 * 1024 * 1024 * 1024, // 1PB (1 petabyte) - practically unlimited
      files: 1,
      headerPairs: 2000,
    },
  });

  app.register(authRoutes);
  app.register(authProvidersRoutes, { prefix: "/auth" });
  app.register(twoFactorRoutes, { prefix: "/auth" });
  app.register(inviteRoutes);
  app.register(userRoutes);
  app.register(folderRoutes);
  app.register(fileRoutes);
  app.register(shareRoutes);
  app.register(reverseShareRoutes);
  app.register(storageRoutes);
  app.register(appRoutes);
  app.register(healthRoutes);
  app.register(s3StorageRoutes);

  if (isInternalStorage) {
    console.log("📦 Using internal storage (auto-configured)");
    if (!env.STORAGE_URL) {
      console.log(
        "⚠️  WARNING: STORAGE_URL not set for internal storage!\n" +
          "   File uploads may fail. Set STORAGE_URL to your public storage URL.\n" +
          "   Example: STORAGE_URL=https://storage.yourdomain.com:9379"
      );
    }
  } else if (isExternalS3) {
    console.log("📦 Using external S3 storage (AWS/S3-compatible)");
    console.log(`   Endpoint: ${storageConfig.endpoint}`);
    console.log(`   Region: ${storageConfig.region}`);
    console.log(`   Bucket: ${storageConfig.bucketName}`);

    // Warn if STORAGE_URL is set for external S3 (common mistake)
    if (env.STORAGE_URL) {
      console.log(
        "⚠️  WARNING: STORAGE_URL is set but not used with external S3 (ENABLE_S3=true)\n" +
          "   STORAGE_URL is only needed for internal storage. You can remove it."
      );
    }
  } else {
    console.log("⚠️  WARNING: Storage not configured! Storage may not work.");
  }

  await app.listen({
    port: 3333,
    host: "0.0.0.0",
  });

  console.log(`🌴 Palmr server running on port 3333`);

  // Cleanup on shutdown
  process.on("SIGINT", () => process.exit(0));
  process.on("SIGTERM", () => process.exit(0));
}

startServer().catch((err) => {
  console.error("Error starting server:", err);
  process.exit(1);
});
