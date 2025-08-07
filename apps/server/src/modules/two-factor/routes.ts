import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { TwoFactorController } from "./controller";

export async function twoFactorRoutes(app: FastifyInstance) {
  const twoFactorController = new TwoFactorController();

  const preValidation = async (request: FastifyRequest, reply: FastifyReply) => {
    try {
      await request.jwtVerify();
    } catch (err) {
      console.error(err);
      reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
    }
  };

  app.post(
    "/2fa/setup",
    {
      preValidation,
      schema: {
        tags: ["Two-Factor Authentication"],
        operationId: "generate2FASetup",
        summary: "Generate 2FA Setup",
        description: "Generate QR code and secret for 2FA setup",
        body: z.object({
          appName: z.string().optional().describe("Application name for QR code"),
        }),
        response: {
          200: z.object({
            secret: z.string().describe("Base32 encoded secret"),
            qrCode: z.string().describe("QR code as data URL"),
            manualEntryKey: z.string().describe("Manual entry key"),
            backupCodes: z
              .array(
                z.object({
                  code: z.string().describe("Backup code"),
                  used: z.boolean().describe("Whether backup code is used"),
                })
              )
              .describe("Backup codes"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    twoFactorController.generateSetup.bind(twoFactorController)
  );

  app.post(
    "/2fa/verify-setup",
    {
      preValidation,
      schema: {
        tags: ["Two-Factor Authentication"],
        operationId: "verify2FASetup",
        summary: "Verify 2FA Setup",
        description: "Verify the setup token and enable 2FA",
        body: z.object({
          token: z.string().min(6).describe("TOTP token"),
          secret: z.string().min(1).describe("Base32 encoded secret"),
        }),
        response: {
          200: z.object({
            success: z.boolean().describe("Setup success"),
            backupCodes: z.array(z.string()).describe("Backup codes"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    twoFactorController.verifySetup.bind(twoFactorController)
  );

  app.post(
    "/2fa/verify",
    {
      preValidation,
      schema: {
        tags: ["Two-Factor Authentication"],
        operationId: "verify2FAToken",
        summary: "Verify 2FA Token",
        description: "Verify a 2FA token during authentication",
        body: z.object({
          token: z.string().min(6).describe("TOTP token or backup code"),
        }),
        response: {
          200: z.object({
            success: z.boolean().describe("Verification success"),
            method: z.enum(["totp", "backup"]).describe("Verification method used"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    twoFactorController.verifyToken.bind(twoFactorController)
  );

  app.post(
    "/2fa/disable",
    {
      preValidation,
      schema: {
        tags: ["Two-Factor Authentication"],
        operationId: "disable2FA",
        summary: "Disable 2FA",
        description: "Disable two-factor authentication",
        body: z.object({
          password: z.string().min(1).describe("User password for confirmation"),
        }),
        response: {
          200: z.object({
            success: z.boolean().describe("Disable success"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    twoFactorController.disable2FA.bind(twoFactorController)
  );

  app.post(
    "/2fa/backup-codes",
    {
      preValidation,
      schema: {
        tags: ["Two-Factor Authentication"],
        operationId: "generateBackupCodes",
        summary: "Generate Backup Codes",
        description: "Generate new backup codes for 2FA",
        response: {
          200: z.object({
            backupCodes: z.array(z.string()).describe("New backup codes"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    twoFactorController.generateBackupCodes.bind(twoFactorController)
  );

  app.get(
    "/2fa/status",
    {
      preValidation,
      schema: {
        tags: ["Two-Factor Authentication"],
        operationId: "get2FAStatus",
        summary: "Get 2FA Status",
        description: "Get current 2FA status for the user",
        response: {
          200: z.object({
            enabled: z.boolean().describe("Whether 2FA is enabled"),
            verified: z.boolean().describe("Whether 2FA is verified"),
            availableBackupCodes: z.number().describe("Number of available backup codes"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    twoFactorController.getStatus.bind(twoFactorController)
  );
}
