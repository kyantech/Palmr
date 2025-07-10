import { FastifyInstance } from "fastify";
import { z } from "zod";

import { prisma } from "../../shared/prisma";
import { AppController } from "./controller";
import { BulkUpdateConfigSchema, ConfigResponseSchema } from "./dto";

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
    "/app/test-smtp",
    {
      preValidation: adminPreValidation,
      schema: {
        tags: ["App"],
        operationId: "testSmtpConnection",
        summary: "Test SMTP connection with provided or saved configuration",
        description:
          "Validates SMTP connectivity using either provided configuration parameters or the currently saved settings. This endpoint allows testing SMTP settings before saving them permanently. Requires admin privileges.",
        body: z
          .object({
            smtpConfig: z
              .object({
                smtpEnabled: z.string().describe("Whether SMTP is enabled ('true' or 'false')"),
                smtpHost: z.string().describe("SMTP server hostname or IP address (e.g., 'smtp.gmail.com')"),
                smtpPort: z
                  .union([z.string(), z.number()])
                  .transform(String)
                  .describe("SMTP server port (typically 587 for TLS, 25 for non-secure)"),
                smtpUser: z.string().describe("Username for SMTP authentication (e.g., email address)"),
                smtpPass: z.string().describe("Password for SMTP authentication (for Gmail, use App Password)"),
                smtpSecure: z
                  .string()
                  .optional()
                  .describe("Connection security method ('auto', 'ssl', 'tls', or 'none')"),
                smtpNoAuth: z.string().optional().describe("Disable SMTP authentication ('true' or 'false')"),
                smtpTrustSelfSigned: z
                  .string()
                  .optional()
                  .describe("Trust self-signed certificates ('true' or 'false')"),
              })
              .optional()
              .describe("SMTP configuration to test. If not provided, uses currently saved configuration"),
          })
          .optional()
          .describe("Request body containing SMTP configuration to test. Send empty body to test saved configuration"),
        response: {
          200: z.object({
            success: z.boolean().describe("Whether the SMTP connection test was successful"),
            message: z.string().describe("Descriptive message about the test result"),
          }),
          400: z.object({
            error: z.string().describe("Error message describing what went wrong with the test"),
          }),
          401: z.object({
            error: z.string().describe("Authentication error - invalid or missing JWT token"),
          }),
          403: z.object({
            error: z.string().describe("Authorization error - user does not have admin privileges"),
          }),
        },
      },
    },
    appController.testSmtpConnection.bind(appController)
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
