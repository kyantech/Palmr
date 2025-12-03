import { FastifyReply, FastifyRequest } from "fastify";

import { InviteService } from "./service";

export class InviteController {
  private inviteService = new InviteService();

  async generateInviteToken(request: FastifyRequest, reply: FastifyReply) {
    try {
      const user = request.user as any;

      if (!user || !user.isAdmin) {
        return reply.status(403).send({ error: "Forbidden: admin access required" });
      }

      const { token, expiresAt } = await this.inviteService.generateInviteToken(user.userId || user.id);
      return reply.send({ token, expiresAt });
    } catch (error) {
      console.error("[Invite Controller] Error generating invite token:", error);
      return reply.status(500).send({ error: "Failed to generate invite token" });
    }
  }

  async validateInviteToken(request: FastifyRequest<{ Params: { token: string } }>, reply: FastifyReply) {
    try {
      const { token } = request.params;
      const validation = await this.inviteService.validateInviteToken(token);

      return reply.send(validation);
    } catch (error) {
      console.error("Error validating invite token:", error);
      return reply.status(500).send({ error: "Failed to validate invite token" });
    }
  }

  async registerWithInvite(
    request: FastifyRequest<{
      Body: {
        token: string;
        firstName: string;
        lastName: string;
        username: string;
        email: string;
        password: string;
      };
    }>,
    reply: FastifyReply
  ) {
    try {
      const { token, firstName, lastName, username, email, password } = request.body;

      const user = await this.inviteService.registerWithInvite({
        token,
        firstName,
        lastName,
        username,
        email,
        password,
      });

      return reply.send({
        message: "User registered successfully",
        user,
      });
    } catch (error: any) {
      console.error("Error registering with invite:", error);

      if (error.message.includes("already been used")) {
        return reply.status(400).send({ error: "This invite link has already been used" });
      }
      if (error.message.includes("expired")) {
        return reply.status(400).send({ error: "This invite link has expired" });
      }
      if (error.message.includes("Invalid invite")) {
        return reply.status(400).send({ error: "Invalid invite link" });
      }
      if (error.message.includes("Username already exists")) {
        return reply.status(400).send({ error: "Username already exists" });
      }
      if (error.message.includes("Email already exists")) {
        return reply.status(400).send({ error: "Email already exists" });
      }

      return reply.status(500).send({ error: "Failed to register user" });
    }
  }
}
