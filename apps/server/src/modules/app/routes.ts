import { prisma } from "shared/prisma";
import { AppController } from "./controller";
import { ConfigResponseSchema, BulkUpdateConfigSchema } from "./dto";
import { FastifyInstance } from "fastify";
import { z } from "zod";

export async function appRoutes(app: FastifyInstance) {
  const appController = new AppController();

  const adminPreValidation = async (request: any, reply: any) => {
    try {
      const usersCount = await prisma.user.count();
      
      if (usersCount <= 1) {
        return;
      }
      
      await request.jwtVerify();
      
      if (!request.user.isAdmin) {
        return reply.status(403).send({
          error: "Access restricted to administrators",
        });
      }
    } catch (err) {
      console.error(err);
      return reply.status(401).send({
        error: ".",
      });
    }
  };

  app.get(
    "/app/info",
    {
      schema: {
        tags: ["App"],
        operationId: "getAppInfo",
        summary: "Get application base information",
        description: "Get application base information",
        response: {
          200: z.object({
            appName: z.string().describe("The application name"),
            appDescription: z.string().describe("The application description"),
            appLogo: z.string().describe("The application logo"),
            firstUserAccess: z.boolean().describe("Whether it's the first user access"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    appController.getAppInfo.bind(appController)
  );

  app.patch(
    "/app/configs/:key",
    {
      preValidation: adminPreValidation,
      schema: {
        tags: ["App"],
        operationId: "updateConfig",
        summary: "Update a configuration value",
        description: "Update a configuration value (admin only)",
        params: z.object({
          key: z.string().describe("The config key"),
        }),
        body: z.object({
          value: z.string().describe("The config value"),
        }),
        response: {
          200: z.object({
            config: ConfigResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          403: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    appController.updateConfig.bind(appController)
  );

  app.get(
    "/app/configs",
    {
      // preValidation: adminPreValidation,
      schema: {
        tags: ["App"],
        operationId: "getAllConfigs",
        summary: "List all configurations",
        description: "List all configurations (admin only)",
        response: {
          200: z.object({
            configs: z.array(ConfigResponseSchema),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          403: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    appController.getAllConfigs.bind(appController)
  );

  app.patch(
    "/app/configs",
    {
      preValidation: adminPreValidation,
      schema: {
        tags: ["App"],
        operationId: "bulkUpdateConfigs",
        summary: "Bulk update configuration values",
        description: "Bulk update configuration values (admin only)",
        body: BulkUpdateConfigSchema,
        response: {
          200: z.object({
            configs: z.array(ConfigResponseSchema),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          403: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    appController.bulkUpdateConfigs.bind(appController)
  );

  app.post(
    "/app/logo",
    {
      preValidation: adminPreValidation,
      schema: {
        tags: ["App"],
        operationId: "uploadLogo",
        summary: "Upload app logo",
        description: "Upload a new app logo (admin only)",
        response: {
          200: z.object({
            logo: z.string().describe("The logo URL"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          403: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    appController.uploadLogo.bind(appController)
  );

  app.delete(
    "/app/logo",
    {
      preValidation: adminPreValidation,
      schema: {
        tags: ["App"],
        operationId: "removeLogo",
        summary: "Remove app logo",
        description: "Remove the current app logo (admin only)",
        response: {
          200: z.object({
            message: z.string().describe("Success message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          403: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    appController.removeLogo.bind(appController)
  );
}
