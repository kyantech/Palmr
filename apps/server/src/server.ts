import crypto from "node:crypto";
import fastifyMultipart from "@fastify/multipart";

import { buildApp } from "./app";
import { appRoutes } from "./modules/app/routes";
import { authProvidersRoutes } from "./modules/auth-providers/routes";
import { authRoutes } from "./modules/auth/routes";
import { bulkDownloadRoutes } from "./modules/bulk-download/routes";
import { fileRoutes } from "./modules/file/routes";
import { folderRoutes } from "./modules/folder/routes";
import { healthRoutes } from "./modules/health/routes";
import { reverseShareRoutes } from "./modules/reverse-share/routes";
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

async function startServer() {
  const app = await buildApp();

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
  app.register(userRoutes);
  app.register(fileRoutes);
  app.register(folderRoutes);
  app.register(shareRoutes);
  app.register(reverseShareRoutes);
  app.register(storageRoutes);
  app.register(bulkDownloadRoutes);
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
  console.log(`🔐 Auth Providers: ${authProviders}`);

  console.log("\n📚 API Documentation:");
  console.log(`   - API Reference: http://localhost:3333/docs\n`);

  process.on("SIGINT", async () => {
    process.exit(0);
  });

  process.on("SIGTERM", async () => {
    process.exit(0);
  });
}

startServer().catch((err) => {
  console.error("Error starting server:", err);
  process.exit(1);
});
