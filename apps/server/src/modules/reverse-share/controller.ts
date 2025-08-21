import { FastifyReply, FastifyRequest } from "fastify";

import {
  CreateReverseShareSchema,
  ReverseSharePasswordSchema,
  UpdateReverseSharePasswordSchema,
  UpdateReverseShareSchema,
  UploadToReverseShareSchema,
} from "./dto";
import { ReverseShareService } from "./service";

export class ReverseShareController {
  private reverseShareService = new ReverseShareService();

  async createReverseShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const input = CreateReverseShareSchema.parse(request.body);
      const reverseShare = await this.reverseShareService.createReverseShare(input, userId);
      return reply.status(201).send({ reverseShare });
    } catch (error: any) {
      console.error("Create Reverse Share Error:", error);
      if (error.errors) {
        return reply.status(400).send({ error: error.errors });
      }
      return reply.status(400).send({ error: error.message || "Unknown error occurred" });
    }
  }

  async listUserReverseShares(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const reverseShares = await this.reverseShareService.listUserReverseShares(userId);
      return reply.send({ reverseShares });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async getReverseShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id } = request.params as { id: string };
      const reverseShare = await this.reverseShareService.getReverseShareById(id, userId);
      return reply.send({ reverseShare });
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to access this reverse share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async getReverseShareForUpload(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { id } = request.params as { id: string };
      const { password } = request.query as { password?: string };

      const reverseShare = await this.reverseShareService.getReverseShareForUpload(id, password);
      return reply.send({ reverseShare });
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Reverse share is inactive") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message === "Reverse share has expired") {
        return reply.status(410).send({ error: error.message });
      }
      if (error.message === "Password required" || error.message === "Invalid password") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async getReverseShareForUploadByAlias(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { alias } = request.params as { alias: string };
      const { password } = request.query as { password?: string };

      const reverseShare = await this.reverseShareService.getReverseShareForUploadByAlias(alias, password);
      return reply.send({ reverseShare });
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Reverse share is inactive") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message === "Reverse share has expired") {
        return reply.status(410).send({ error: error.message });
      }
      if (error.message === "Password required" || error.message === "Invalid password") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async updateReverseShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id, ...updateData } = UpdateReverseShareSchema.parse(request.body);
      const reverseShare = await this.reverseShareService.updateReverseShare(id, updateData, userId);
      return reply.send({ reverseShare });
    } catch (error: any) {
      console.error("Update Reverse Share Error:", error);
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to update this reverse share") {
        return reply.status(401).send({ error: error.message });
      }
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

      const { id } = request.params as { id: string };
      const { password } = UpdateReverseSharePasswordSchema.parse(request.body);

      const updateData: { password?: string | null } = { password };
      const reverseShare = await this.reverseShareService.updateReverseShare(id, updateData, userId);
      return reply.send({ reverseShare });
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to update this reverse share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async deleteReverseShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id } = request.params as { id: string };
      const reverseShare = await this.reverseShareService.deleteReverseShare(id, userId);
      return reply.send({ reverseShare });
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to delete this reverse share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async getPresignedUrl(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { id } = request.params as { id: string };
      const { password } = request.query as { password?: string };
      const { objectName } = request.body as { objectName: string };

      const result = await this.reverseShareService.getPresignedUrl(id, objectName, password);
      return reply.send(result);
    } catch (error: any) {
      console.error("Get Presigned URL Error:", error);
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Reverse share is inactive") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message === "Reverse share has expired") {
        return reply.status(410).send({ error: error.message });
      }
      if (error.message === "Password required" || error.message === "Invalid password") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async getPresignedUrlByAlias(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { alias } = request.params as { alias: string };
      const { password } = request.query as { password?: string };
      const { objectName } = request.body as { objectName: string };

      const result = await this.reverseShareService.getPresignedUrlByAlias(alias, objectName, password);
      return reply.send(result);
    } catch (error: any) {
      console.error("Get Presigned URL by Alias Error:", error);
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Reverse share is inactive") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message === "Reverse share has expired") {
        return reply.status(410).send({ error: error.message });
      }
      if (error.message === "Password required" || error.message === "Invalid password") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async registerFileUpload(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { id } = request.params as { id: string };
      const { password } = request.query as { password?: string };
      const fileData = UploadToReverseShareSchema.parse(request.body);

      const file = await this.reverseShareService.registerFileUpload(id, fileData, password);
      return reply.status(201).send({ file });
    } catch (error: any) {
      console.error("Register File Upload Error:", error);
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Reverse share is inactive") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message === "Reverse share has expired") {
        return reply.status(410).send({ error: error.message });
      }
      if (error.message === "Password required" || error.message === "Invalid password") {
        return reply.status(401).send({ error: error.message });
      }
      if (error.message === "Maximum number of files reached") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message.includes("File type") && error.message.includes("not allowed")) {
        return reply.status(400).send({ error: error.message });
      }
      if (error.message === "File size exceeds limit") {
        return reply.status(400).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async registerFileUploadByAlias(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { alias } = request.params as { alias: string };
      const { password } = request.query as { password?: string };
      const fileData = UploadToReverseShareSchema.parse(request.body);

      const file = await this.reverseShareService.registerFileUploadByAlias(alias, fileData, password);
      return reply.status(201).send({ file });
    } catch (error: any) {
      console.error("Register File Upload by Alias Error:", error);
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Reverse share is inactive") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message === "Reverse share has expired") {
        return reply.status(410).send({ error: error.message });
      }
      if (error.message === "Password required" || error.message === "Invalid password") {
        return reply.status(401).send({ error: error.message });
      }
      if (error.message === "Maximum number of files reached") {
        return reply.status(403).send({ error: error.message });
      }
      if (error.message.includes("File type") && error.message.includes("not allowed")) {
        return reply.status(400).send({ error: error.message });
      }
      if (error.message === "File size exceeds limit") {
        return reply.status(400).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async downloadFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { fileId } = request.params as { fileId: string };

      const fileInfo = await this.reverseShareService.getFileInfo(fileId, userId);
      const downloadId = `reverse-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;

      const { DownloadMemoryManager } = await import("../../utils/download-memory-manager.js");
      const memoryManager = DownloadMemoryManager.getInstance();

      const fileSizeMB = Number(fileInfo.size) / (1024 * 1024);
      console.log(
        `[REVERSE-DOWNLOAD] Requesting slot for ${downloadId}: ${fileInfo.name} (${fileSizeMB.toFixed(1)}MB)`
      );

      try {
        await memoryManager.requestDownloadSlot(downloadId, {
          fileName: fileInfo.name,
          fileSize: Number(fileInfo.size),
          objectName: fileInfo.objectName,
        });
      } catch (error: any) {
        console.warn(`[REVERSE-DOWNLOAD] Queued ${downloadId}: ${error.message}`);
        return reply.status(202).send({
          queued: true,
          downloadId: downloadId,
          message: "Download queued due to memory constraints",
          estimatedWaitTime: error.estimatedWaitTime || 60,
        });
      }

      console.log(`[REVERSE-DOWNLOAD] Starting ${downloadId}: ${fileInfo.name} (${fileSizeMB.toFixed(1)}MB)`);
      memoryManager.startDownload(downloadId);

      try {
        const result = await this.reverseShareService.downloadReverseShareFile(fileId, userId);

        const originalUrl = result.url;
        reply.header("X-Download-ID", downloadId);

        reply.raw.on("finish", () => {
          memoryManager.endDownload(downloadId);
        });

        reply.raw.on("close", () => {
          memoryManager.endDownload(downloadId);
        });

        reply.raw.on("error", () => {
          memoryManager.endDownload(downloadId);
        });

        return reply.send(result);
      } catch (downloadError) {
        memoryManager.endDownload(downloadId);
        throw downloadError;
      }
    } catch (error: any) {
      if (error.message === "File not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to download this file") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async deleteFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { fileId } = request.params as { fileId: string };
      const file = await this.reverseShareService.deleteReverseShareFile(fileId, userId);
      return reply.send({ file });
    } catch (error: any) {
      if (error.message === "File not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to delete this file") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async checkPassword(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { id } = request.params as { id: string };
      const { password } = ReverseSharePasswordSchema.parse(request.body);

      const result = await this.reverseShareService.checkPassword(id, password);
      return reply.send(result);
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async activateReverseShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id } = request.params as { id: string };
      const reverseShare = await this.reverseShareService.activateReverseShare(id, userId);
      return reply.send({ reverseShare });
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to activate this reverse share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async deactivateReverseShare(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id } = request.params as { id: string };
      const reverseShare = await this.reverseShareService.deactivateReverseShare(id, userId);
      return reply.send({ reverseShare });
    } catch (error: any) {
      if (error.message === "Reverse share not found") {
        return reply.status(404).send({ error: error.message });
      }
      if (error.message === "Unauthorized to deactivate this reverse share") {
        return reply.status(401).send({ error: error.message });
      }
      return reply.status(400).send({ error: error.message });
    }
  }

  async createOrUpdateAlias(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { reverseShareId } = request.params as { reverseShareId: string };
      const { alias } = request.body as { alias: string };
      const userId = (request as any).user.userId;

      const result = await this.reverseShareService.createOrUpdateAlias(reverseShareId, alias, userId);
      return reply.send({ alias: result });
    } catch (error: any) {
      return reply.status(400).send({ error: error.message });
    }
  }

  async updateFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const { fileId } = request.params as { fileId: string };
      const body = request.body as { name?: string; description?: string | null };
      const userId = (request as any).user?.userId;

      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      const file = await this.reverseShareService.updateReverseShareFile(fileId, body, userId);
      return reply.send({ file });
    } catch (error: any) {
      if (error.message === "File not found") {
        return reply.status(404).send({ error: "File not found" });
      }
      if (error.message === "Unauthorized to edit this file") {
        return reply.status(403).send({ error: "Unauthorized to edit this file" });
      }
      console.error("Error in updateFile:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  async copyFileToUserFiles(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();

      const { fileId } = request.params as { fileId: string };
      const userId = (request as any).user?.userId;

      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized" });
      }

      console.log(`Copy to my files: User ${userId} copying file ${fileId}`);

      const file = await this.reverseShareService.copyReverseShareFileToUserFiles(fileId, userId);

      console.log(`Copy to my files: Successfully copied file ${fileId}`);

      return reply.send({ file, message: "File copied to your files successfully" });
    } catch (error: any) {
      console.error(`Copy to my files: Error:`, error.message);

      if (error.message === "File not found") {
        return reply.status(404).send({ error: "File not found" });
      }
      if (error.message === "Unauthorized to copy this file") {
        return reply.status(403).send({ error: "Unauthorized to copy this file" });
      }
      if (error.message.includes("File size exceeds") || error.message.includes("Insufficient storage")) {
        return reply.status(400).send({ error: error.message });
      }
      console.error("Error in copyFileToUserFiles:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }
}
