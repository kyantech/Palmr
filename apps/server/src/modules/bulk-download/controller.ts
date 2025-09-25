import { GetObjectCommand } from "@aws-sdk/client-s3";
import { getSignedUrl } from "@aws-sdk/s3-request-presigner";
import archiver from "archiver";
import { FastifyReply, FastifyRequest } from "fastify";

import { bucketName, s3Client } from "../../config/storage.config";
import { prisma } from "../../shared/prisma";
import { ReverseShareService } from "../reverse-share/service";

export class BulkDownloadController {
  private reverseShareService = new ReverseShareService();

  async downloadFiles(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { fileIds, folderIds, zipName } = request.body as {
        fileIds: string[];
        folderIds: string[];
        zipName: string;
      };

      if (!fileIds.length && !folderIds.length) {
        return reply.status(400).send({ error: "No files or folders to download" });
      }

      // Get files from database
      const files = await prisma.file.findMany({
        where: {
          id: { in: fileIds },
        },
        select: {
          id: true,
          name: true,
          objectName: true,
          size: true,
        },
      });

      // Get folders and their files
      const folders = await prisma.folder.findMany({
        where: {
          id: { in: folderIds },
        },
        include: {
          files: {
            select: {
              id: true,
              name: true,
              objectName: true,
              size: true,
            },
          },
        },
      });

      // Create ZIP stream
      const archive = archiver("zip", {
        zlib: { level: 9 },
      });

      reply.raw.setHeader("Content-Type", "application/zip");
      reply.raw.setHeader("Content-Disposition", `attachment; filename="${zipName}"`);
      reply.raw.setHeader("Transfer-Encoding", "chunked");

      archive.pipe(reply.raw);

      // Add files to ZIP
      for (const file of files) {
        try {
          const downloadUrl = await getSignedUrl(
            s3Client,
            new GetObjectCommand({
              Bucket: bucketName,
              Key: file.objectName,
            }),
            { expiresIn: 300 } // 5 minutes
          );

          const response = await fetch(downloadUrl);
          if (response.ok) {
            const buffer = await response.arrayBuffer();
            archive.append(Buffer.from(buffer), { name: file.name });
          }
        } catch (error) {
          console.error(`Error downloading file ${file.name}:`, error);
        }
      }

      // Add folder files to ZIP
      for (const folder of folders) {
        for (const file of folder.files) {
          try {
            const downloadUrl = await getSignedUrl(
              s3Client,
              new GetObjectCommand({
                Bucket: bucketName,
                Key: file.objectName,
              }),
              { expiresIn: 300 }
            );

            const response = await fetch(downloadUrl);
            if (response.ok) {
              const buffer = await response.arrayBuffer();
              archive.append(Buffer.from(buffer), {
                name: `${folder.name}/${file.name}`,
              });
            }
          } catch (error) {
            console.error(`Error downloading file ${file.name}:`, error);
          }
        }
      }

      await archive.finalize();
    } catch (error) {
      console.error("Bulk download error:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  async downloadFolder(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { folderId, folderName } = request.params as {
        folderId: string;
        folderName: string;
      };

      // Get folder and all its files recursively
      const folder = await prisma.folder.findUnique({
        where: { id: folderId },
        include: {
          files: {
            select: {
              id: true,
              name: true,
              objectName: true,
              size: true,
            },
          },
        },
      });

      if (!folder) {
        return reply.status(404).send({ error: "Folder not found" });
      }

      // Create ZIP stream
      const archive = archiver("zip", {
        zlib: { level: 9 },
      });

      reply.raw.setHeader("Content-Type", "application/zip");
      reply.raw.setHeader("Content-Disposition", `attachment; filename="${folderName}.zip"`);
      reply.raw.setHeader("Transfer-Encoding", "chunked");

      archive.pipe(reply.raw);

      // Add all files to ZIP
      for (const file of folder.files) {
        try {
          const downloadUrl = await getSignedUrl(
            s3Client,
            new GetObjectCommand({
              Bucket: bucketName,
              Key: file.objectName,
            }),
            { expiresIn: 300 }
          );

          const response = await fetch(downloadUrl);
          if (response.ok) {
            const buffer = await response.arrayBuffer();
            archive.append(Buffer.from(buffer), { name: file.name });
          }
        } catch (error) {
          console.error(`Error downloading file ${file.name}:`, error);
        }
      }

      await archive.finalize();
    } catch (error) {
      console.error("Folder download error:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  async downloadReverseShareFiles(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { fileIds, zipName } = request.body as {
        fileIds: string[];
        zipName: string;
      };

      if (!fileIds.length) {
        return reply.status(400).send({ error: "No files to download" });
      }

      // Get reverse share files from database
      const files = await prisma.reverseShareFile.findMany({
        where: {
          id: { in: fileIds },
          reverseShare: {
            creatorId: userId, // Only allow creator to download
          },
        },
        select: {
          id: true,
          name: true,
          objectName: true,
          size: true,
        },
      });

      if (files.length === 0) {
        return reply.status(404).send({ error: "No files found or unauthorized" });
      }

      // Create ZIP stream
      const archive = archiver("zip", {
        zlib: { level: 9 },
      });

      reply.raw.setHeader("Content-Type", "application/zip");
      reply.raw.setHeader("Content-Disposition", `attachment; filename="${zipName}"`);
      reply.raw.setHeader("Transfer-Encoding", "chunked");

      archive.pipe(reply.raw);

      // Add files to ZIP
      for (const file of files) {
        try {
          const downloadUrl = await getSignedUrl(
            s3Client,
            new GetObjectCommand({
              Bucket: bucketName,
              Key: file.objectName,
            }),
            { expiresIn: 300 } // 5 minutes
          );

          const response = await fetch(downloadUrl);
          if (response.ok) {
            const buffer = await response.arrayBuffer();
            archive.append(Buffer.from(buffer), { name: file.name });
          }
        } catch (error) {
          console.error(`Error downloading reverse share file ${file.name}:`, error);
        }
      }

      await archive.finalize();
    } catch (error) {
      console.error("Reverse share bulk download error:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }
}
