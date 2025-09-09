import { FastifyReply, FastifyRequest } from "fastify";

import {
  CreateShareSchema,
  UpdateShareItemsSchema,
  UpdateSharePasswordSchema,
  UpdateShareRecipientsSchema,
  UpdateShareSchema,
} from "./dto";
import { ShareService } from "./service";

export class ShareController {
  private shareService = new ShareService();

  async createShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const input = CreateShareSchema.parse(request.body);
      const share = await this.shareService.createShare(input, userId);
      return reply.status(201).send({ share });
    } catch (error: any) {
      console.error("Create Share Error:", error);
      if (error.errors) {
        return reply.status(400).send({ error: error.errors });
      }
      return reply.status(400).send({ error: error.message || "Unknown error occurred" });
    }
  }

  async listUserShares(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const shares = await this.shareService.listUserShares(userId);
      return reply.send({ shares });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async getShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { shareId } = request.params as { shareId: string };
      const { password } = request.query as { password?: string };

      let userId: string | undefined;
      try {
        await request.jwtVerify();
        userId = (request as any).user?.userId;
      } catch (err) {
        console.error(err);
      }

      const share = await this.shareService.getShare(shareId, password, userId);
      return reply.send({ share });
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Share has reached maximum views") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message === "Share has expired") {
        return reply.status(410).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async updateShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const { id, ...updateData } = UpdateShareSchema.parse(request.body);
      const share = await this.shareService.updateShare(id, updateData, userId);
      return reply.send({ share });
    } catch (error: any) {
      console.error("Update Share Error:", error);
      return reply.status(400).send({ error: error.message });
    }
  }

  async updatePassword(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { shareId } = request.params as { shareId: string };
      const { password } = UpdateSharePasswordSchema.parse(request.body);

      const share = await this.shareService.updateSharePassword(shareId, userId, password);
      return reply.send({ share });
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to update this share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async addItems(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { shareId } = request.params as { shareId: string };
      const { files, folders } = UpdateShareItemsSchema.parse(request.body);

      const share = await this.shareService.addItemsToShare(shareId, userId, files || [], folders || []);
      return reply.send({ share });
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to update this share") {
        return reply.status(401).send({ error: error.message });
      }
      if (error.message.startsWith("Files not found:") || error.message.startsWith("Folders not found:")) {
        return reply.status(404).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async removeItems(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { shareId } = request.params as { shareId: string };
      const { files, folders } = UpdateShareItemsSchema.parse(request.body);

      const share = await this.shareService.removeItemsFromShare(shareId, userId, files || [], folders || []);
      return reply.send({ share });
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to update this share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async deleteShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id } = request.params as { id: string };

      const share = await this.shareService.findShareById(id);
      if (!share) {
        return reply.status(404).send({ error: "Share not found" });
      }

      if (share.creatorId !== userId) {
        return reply.status(401).send({ error: "Unauthorized to delete this share" });
      }

      const deleted = await this.shareService.deleteShare(id);
      return reply.send({ share: deleted });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async addRecipients(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { shareId } = request.params as { shareId: string };
      const { emails } = UpdateShareRecipientsSchema.parse(request.body);

      const share = await this.shareService.addRecipients(shareId, userId, emails);
      return reply.send({ share });
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to update this share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async removeRecipients(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { shareId } = request.params as { shareId: string };
      const { emails } = UpdateShareRecipientsSchema.parse(request.body);

      const share = await this.shareService.removeRecipients(shareId, userId, emails);
      return reply.send({ share });
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to update this share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async createOrUpdateAlias(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { shareId } = request.params as { shareId: string };
      const { alias } = request.body as { alias: string };
      const userId = (request as any).user.userId;

      const result = await this.shareService.createOrUpdateAlias(shareId, alias, userId);
      return reply.send({ alias: result });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async getShareByAlias(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { alias } = request.params as { alias: string };
      const { password } = request.query as { password?: string };

      const share = await this.shareService.getShareByAlias(alias, password);
      return reply.send({ share });
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async notifyRecipients(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { shareId } = request.params as { shareId: string };
      const { shareLink } = request.body as { shareLink: string };

      const result = await this.shareService.notifyRecipients(shareId, userId, shareLink);
      return reply.send(result);
    } catch (error: any) {
      if (error.message === "Share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to access this share") {
        return reply.status(401).send({ error: error.message });
      }
      if (error.message === "SMTP is not enabled") {
        return reply.status(400).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }
}
