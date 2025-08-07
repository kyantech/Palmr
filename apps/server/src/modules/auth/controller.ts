import { FastifyReply, FastifyRequest } from "fastify";

import { env } from "../../env";
import { ConfigService } from "../config/service";
import {
  CompleteTwoFactorLoginSchema,
  createResetPasswordSchema,
  LoginSchema,
  RequestPasswordResetSchema,
} from "./dto";
import { AuthService } from "./service";

export class AuthController {
  private authService = new AuthService();
  private configService = new ConfigService();

  private getClientInfo(request: FastifyRequest) {
    const realIP = request.headers["x-real-ip"] as string;
    const realUserAgent = request.headers["x-user-agent"] as string;

    const userAgent = realUserAgent || request.headers["user-agent"] || "";
    const ipAddress = realIP || request.ip || request.socket.remoteAddress || "";

    return { userAgent, ipAddress };
  }

  async login(request: FastifyRequest, reply: FastifyReply) {
    try {
      const input = LoginSchema.parse(request.body);
      const { userAgent, ipAddress } = this.getClientInfo(request);
      const result = await this.authService.login(input, userAgent, ipAddress);

      if ("requiresTwoFactor" in result) {
        return reply.send(result);
      }

      const user = result;
      const token = await request.jwtSign({
        userId: user.id,
        isAdmin: user.isAdmin,
      });

      reply.setCookie("token", token, {
        httpOnly: true,
        path: "/",
        secure: env.SECURE_SITE === "true" ? true : false,
        sameSite: env.SECURE_SITE === "true" ? "lax" : "strict",
      });

      return reply.send({ user });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async completeTwoFactorLogin(request: FastifyRequest, reply: FastifyReply) {
    try {
      const input = CompleteTwoFactorLoginSchema.parse(request.body);
      const { userAgent, ipAddress } = this.getClientInfo(request);
      const user = await this.authService.completeTwoFactorLogin(
        input.userId,
        input.token,
        input.rememberDevice,
        userAgent,
        ipAddress
      );

      const token = await request.jwtSign({
        userId: user.id,
        isAdmin: user.isAdmin,
      });

      reply.setCookie("token", token, {
        httpOnly: true,
        path: "/",
        secure: env.SECURE_SITE === "true" ? true : false,
        sameSite: env.SECURE_SITE === "true" ? "lax" : "strict",
      });

      return reply.send({ user });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async logout(request: FastifyRequest, reply: FastifyReply) {
    reply.clearCookie("token", { path: "/" });
    return reply.send({ message: "Logout successful" });
  }

  async requestPasswordReset(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { email, origin } = RequestPasswordResetSchema.parse(request.body);
      await this.authService.requestPasswordReset(email, origin);
      return reply.send({
        message: "If an account exists with this email, a password reset link will be sent.",
      });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async resetPassword(request: FastifyRequest, reply: FastifyReply) {
    try {
      const schema = await createResetPasswordSchema();
      const input = schema.parse(request.body);
      await this.authService.resetPassword(input.token, input.password);
      return reply.send({ message: "Password reset successfully" });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async getCurrentUser(request: FastifyRequest, reply: FastifyReply) {
    try {
      let userId: string | null = null;
      try {
        await request.jwtVerify();
        userId = (request as any).user?.userId;
      } catch (err) {
        return reply.send({ user: null });
      }

      if (!userId) {
        return reply.send({ user: null });
      }

      const user = await this.authService.getUserById(userId);
      if (!user) {
        return reply.send({ user: null });
      }

      return reply.send({ user });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async getTrustedDevices(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const devices = await this.authService.getTrustedDevices(userId);
      return reply.send({ devices });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async removeTrustedDevice(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id } = request.params as { id: string };
      await this.authService.removeTrustedDevice(userId, id);
      return reply.send({ success: true, message: "Trusted device removed successfully" });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async removeAllTrustedDevices(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const result = await this.authService.removeAllTrustedDevices(userId);
      return reply.send(result);
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async getAuthConfig(request: FastifyRequest, reply: FastifyReply) {
    try {
      const passwordAuthEnabled = await this.configService.getValue("passwordAuthEnabled");
      return reply.send({
        passwordAuthEnabled: passwordAuthEnabled === "true",
      });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }
}
