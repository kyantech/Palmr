/**
 * S3 Storage Controller (Simplified)
 *
 * This controller handles uploads/downloads using S3-compatible storage (Garage).
 * It's much simpler than the filesystem controller because:
 * - Uses S3 multipart uploads (no chunk management needed)
 * - Uses presigned URLs (no streaming through Node.js)
 * - No memory management needed (Garage handles it)
 * - No encryption needed (Garage handles it)
 *
 * Replaces ~800 lines of complex code with ~100 lines of simple code.
 */

import { FastifyReply, FastifyRequest } from "fastify";

import { S3StorageProvider } from "../../providers/s3-storage.provider";

export class S3StorageController {
  private storageProvider = new S3StorageProvider();

  /**
   * Generate presigned upload URL
   * Client uploads directly to S3 (Garage)
   */
  async getUploadUrl(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { objectName, expires } = request.body as { objectName: string; expires?: number };

      if (!objectName) {
        return reply.status(400).send({ error: "objectName is required" });
      }

      const expiresIn = expires || 3600; // 1 hour default

      // Import storage config to check if using internal or external S3
      const { isInternalStorage } = await import("../../config/storage.config.js");

      let uploadUrl: string;

      if (isInternalStorage) {
        // Internal storage: Use frontend proxy (much simpler!)
        uploadUrl = `/api/files/upload?objectName=${encodeURIComponent(objectName)}`;
      } else {
        // External S3: Use presigned URLs directly (more efficient)
        uploadUrl = await this.storageProvider.getPresignedPutUrl(objectName, expiresIn);
      }

      return reply.status(200).send({
        uploadUrl,
        objectName,
        expiresIn,
        message: isInternalStorage ? "Upload via backend proxy" : "Upload directly to this URL using PUT request",
      });
    } catch (error) {
      console.error("[S3] Error generating upload URL:", error);
      return reply.status(500).send({ error: "Failed to generate upload URL" });
    }
  }

  /**
   * Generate presigned download URL
   * For internal storage: Uses backend proxy
   * For external S3: Uses presigned URLs directly
   */
  async getDownloadUrl(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { objectName, expires, fileName } = request.query as {
        objectName: string;
        expires?: string;
        fileName?: string;
      };

      if (!objectName) {
        return reply.status(400).send({ error: "objectName is required" });
      }

      // Check if file exists
      const exists = await this.storageProvider.fileExists(objectName);
      if (!exists) {
        return reply.status(404).send({ error: "File not found" });
      }

      const expiresIn = expires ? parseInt(expires, 10) : 3600;

      // Import storage config to check if using internal or external S3
      const { isInternalStorage } = await import("../../config/storage.config.js");

      let downloadUrl: string;

      if (isInternalStorage) {
        // Internal storage: Use frontend proxy (much simpler!)
        downloadUrl = `/api/files/download?objectName=${encodeURIComponent(objectName)}`;
      } else {
        // External S3: Use presigned URLs directly (more efficient)
        downloadUrl = await this.storageProvider.getPresignedGetUrl(objectName, expiresIn, fileName);
      }

      return reply.status(200).send({
        downloadUrl,
        objectName,
        expiresIn,
        message: isInternalStorage ? "Download via backend proxy" : "Download directly from this URL",
      });
    } catch (error) {
      console.error("[S3] Error generating download URL:", error);
      return reply.status(500).send({ error: "Failed to generate download URL" });
    }
  }

  /**
   * Upload directly (for small files)
   * Receives file and uploads to S3
   */
  async upload(request: FastifyRequest, reply: FastifyReply) {
    try {
      // For large files, clients should use presigned URLs
      // This is just for backward compatibility or small files

      return reply.status(501).send({
        error: "Not implemented",
        message: "Use getUploadUrl endpoint for efficient uploads",
      });
    } catch (error) {
      console.error("[S3] Error in upload:", error);
      return reply.status(500).send({ error: "Upload failed" });
    }
  }

  /**
   * Delete object from S3
   */
  async deleteObject(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { objectName } = request.params as { objectName: string };

      if (!objectName) {
        return reply.status(400).send({ error: "objectName is required" });
      }

      await this.storageProvider.deleteObject(objectName);

      return reply.status(200).send({
        message: "Object deleted successfully",
        objectName,
      });
    } catch (error) {
      console.error("[S3] Error deleting object:", error);
      return reply.status(500).send({ error: "Failed to delete object" });
    }
  }

  /**
   * Check if object exists
   */
  async checkExists(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { objectName } = request.query as { objectName: string };

      if (!objectName) {
        return reply.status(400).send({ error: "objectName is required" });
      }

      const exists = await this.storageProvider.fileExists(objectName);

      return reply.status(200).send({
        exists,
        objectName,
      });
    } catch (error) {
      console.error("[S3] Error checking existence:", error);
      return reply.status(500).send({ error: "Failed to check existence" });
    }
  }
}
