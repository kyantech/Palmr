import { FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { prisma } from "../../shared/prisma";
import { ConfigService } from "../config/service";
import { TwoFactorService } from "./service";

const SetupSchema = z
  .object({
    appName: z.string().optional(),
  })
  .optional()
  .default({});

const VerifySetupSchema = z.object({
  token: z.string().min(6, "Token must be at least 6 characters"),
  secret: z.string().min(1, "Secret is required"),
});

const VerifyTokenSchema = z.object({
  token: z.string().min(6, "Token must be at least 6 characters"),
});

const DisableSchema = z.object({
  password: z.string().min(1, "Password is required"),
});

export class TwoFactorController {
  private twoFactorService = new TwoFactorService();
  private configService = new ConfigService();

  /**
   * Generate 2FA setup (QR code and secret)
   */
  async generateSetup(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const body = SetupSchema.parse(request.body || {});

      const user = await prisma.user.findUnique({
        where: { id: userId },
        select: { email: true },
      });

      if (!user) {
        return reply.status(404).send({ error: "User not found" });
      }

      const appName = body?.appName || (await this.configService.getValue("appName")) || "Palmr";

      const setupData = await this.twoFactorService.generateSetup(userId, user.email, appName);

      return reply.send(setupData);
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  /**
   * Verify setup token and enable 2FA
   */
  async verifySetup(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const body = VerifySetupSchema.parse(request.body);

      const result = await this.twoFactorService.verifySetup(userId, body.token, body.secret);

      return reply.send(result);
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  /**
   * Verify 2FA token during login
   */
  async verifyToken(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const body = VerifyTokenSchema.parse(request.body);

      const result = await this.twoFactorService.verifyToken(userId, body.token);

      return reply.send(result);
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  /**
   * Disable 2FA
   */
  async disable2FA(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const body = DisableSchema.parse(request.body);

      const result = await this.twoFactorService.disable2FA(userId, body.password);

      return reply.send(result);
    } catch (error: any) {
      console.error("2FA Disable Error:", error.message);
      return reply.status(400).send({ error: error.message });
    }
  }

  /**
   * Generate new backup codes
   */
  async generateBackupCodes(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const codes = await this.twoFactorService.generateNewBackupCodes(userId);

      return reply.send({ backupCodes: codes });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  /**
   * Get 2FA status
   */
  async getStatus(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const status = await this.twoFactorService.getStatus(userId);

      return reply.send(status);
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }
}
