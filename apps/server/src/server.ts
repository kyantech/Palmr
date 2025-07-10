import * as fs from "fs/promises";
import crypto from "node:crypto";
import path from "path";
import fastifyMultipart from "@fastify/multipart";
import fastifyStatic from "@fastify/static";

import { buildApp } from "./app";
import { directoriesConfig } from "./config/directories.config";
import { env } from "./env";
import { appRoutes } from "./modules/app/routes";
import { authProvidersRoutes } from "./modules/auth-providers/routes";
import { authRoutes } from "./modules/auth/routes";
import { fileRoutes } from "./modules/file/routes";
import { filesystemRoutes } from "./modules/filesystem/routes";
import { healthRoutes } from "./modules/health/routes";
import { reverseShareRoutes } from "./modules/reverse-share/routes";
import { shareRoutes } from "./modules/share/routes";
import { storageRoutes } from "./modules/storage/routes";
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

  if (env.ENABLE_S3 !== "true") {
    await app.register(fastifyStatic, {
      root: directoriesConfig.uploads,
      prefix: "/uploads/",
      decorateReply: false,
    });
  }

  app.register(authRoutes);
  app.register(authProvidersRoutes, { prefix: "/auth" });
  app.register(userRoutes);
  app.register(fileRoutes);

  if (env.ENABLE_S3 !== "true") {
    app.register(filesystemRoutes);
  }

  app.register(shareRoutes);
  app.register(reverseShareRoutes);
  app.register(storageRoutes);
  app.register(appRoutes);
  app.register(healthRoutes);

  await app.listen({
    port: 3333,
    host: "0.0.0.0",
  });

  let authProviders = "Disabled";
  try {
    const { AuthProvidersService } = await import("./modules/auth-providers/service.js");
    const authService = new AuthProvidersService();
    const enabledProviders = await authService.getEnabledProviders();
    authProviders = enabledProviders.length > 0 ? `Enabled (${enabledProviders.length} providers)` : "Disabled";
  } catch (error) {
    console.error("Error getting auth providers status:", error);
  }

  console.log(`🌴 Palmr server running on port 3333 🌴`);
  console.log(`📦 Storage mode: ${env.ENABLE_S3 === "true" ? "S3" : "Local Filesystem (Encrypted)"}`);
  console.log(`🔐 Auth Providers: ${authProviders}`);

  console.log("\n📚 API Documentation:");
  console.log(`   - API Reference: http://localhost:3333/docs\n`);
}

startServer().catch((err) => {
  console.error("Error starting server:", err);
  process.exit(1);
});
