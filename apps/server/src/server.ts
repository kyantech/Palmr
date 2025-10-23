import * as fs from "fs/promises";
import crypto from "node:crypto";
import path from "path";
import fastifyMultipart from "@fastify/multipart";

import { buildApp } from "./app";
import { directoriesConfig } from "./config/directories.config";
import { env } from "./env";
import { appRoutes } from "./modules/app/routes";
import { authProvidersRoutes } from "./modules/auth-providers/routes";
import { authRoutes } from "./modules/auth/routes";
import { fileRoutes } from "./modules/file/routes";
import { folderRoutes } from "./modules/folder/routes";
import { healthRoutes } from "./modules/health/routes";
import { reverseShareRoutes } from "./modules/reverse-share/routes";
import { s3StorageRoutes } from "./modules/s3-storage/routes";
import { shareRoutes } from "./modules/share/routes";
import { storageRoutes } from "./modules/storage/routes";
import { twoFactorRoutes } from "./modules/two-factor/routes";
import { userRoutes } from "./modules/user/routes";
import { IS_RUNNING_IN_CONTAINER } from "./utils/container-detection";

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

  // Import storage config once at the beginning
  const { isInternalStorage, isExternalS3 } = await import("./config/storage.config.js");

  // Run automatic migration from legacy storage to S3-compatible storage
  // Transparently migrates any existing files
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

  // No static files needed - S3 serves files directly via presigned URLs

  app.register(authRoutes);
  app.register(authProvidersRoutes, { prefix: "/auth" });
  app.register(twoFactorRoutes, { prefix: "/auth" });
  app.register(userRoutes);
  app.register(folderRoutes);
  app.register(fileRoutes);
  app.register(shareRoutes);
  app.register(reverseShareRoutes);
  app.register(storageRoutes);
  app.register(appRoutes);
  app.register(healthRoutes);

  // Always use S3-compatible storage routes
  app.register(s3StorageRoutes);

  if (isInternalStorage) {
    console.log("📦 Using internal storage (auto-configured)");
  } else if (isExternalS3) {
    console.log("📦 Using external S3 storage (AWS/S3-compatible/etc)");
  } else {
    console.log("⚠️  WARNING: Storage not configured! Storage may not work.");
  }

  await app.listen({
    port: 3333,
    host: "0.0.0.0",
  });

  const storageMode = isInternalStorage ? "Internal Storage" : isExternalS3 ? "External S3" : "Not Configured";

  console.log(`🌴 Palmr server running on port 3333 🌴`);
  console.log(`📦 Storage: ${storageMode}`);

  // Cleanup on shutdown
  process.on("SIGINT", () => process.exit(0));
  process.on("SIGTERM", () => process.exit(0));
}

startServer().catch((err) => {
  console.error("Error starting server:", err);
  process.exit(1);
});
