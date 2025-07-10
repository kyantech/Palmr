import { FastifyReply, FastifyRequest } from "fastify";

import { env } from "../../env";
import { createResetPasswordSchema, LoginSchema, RequestPasswordResetSchema } from "./dto";
import { AuthService } from "./service";

export class AuthController {
  private authService = new AuthService();

  async login(request: FastifyRequest, reply: FastifyReply) {
    try {
      const input = LoginSchema.parse(request.body);
      const user = await this.authService.login(input);
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
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const user = await this.authService.getUserById(userId);
      if (!user) {
        return reply.status(404).send({ error: "User not found" });
      }

      return reply.send({ user });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }
}
