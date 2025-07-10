import { FastifyReply, FastifyRequest } from "fastify";

import { prisma } from "../../shared/prisma";
import { ConfigService } from "../config/service";
import { CheckFileInput, CheckFileSchema, RegisterFileInput, RegisterFileSchema, UpdateFileSchema } from "./dto";
import { FileService } from "./service";

export class FileController {
  private fileService = new FileService();
  private configService = new ConfigService();

  async getPresignedUrl(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { filename, extension } = request.query as {
        filename?: string;
        extension?: string;
      };
      if (!filename || !extension) {
        return reply.status(400).send({
          error: "The 'filename' and 'extension' parameters are required.",
        });
      }

      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const objectName = `${userId}/${Date.now()}-${filename}.${extension}`;
      const expires = 3600;

      const url = await this.fileService.getPresignedPutUrl(objectName, expires);
      return reply.send({ url, objectName });
    } catch (error) {
      console.error("Error in getPresignedUrl:", error);
      return reply.status(500).send({ error: "Internal server error." });
    }
  }

  async registerFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const input: RegisterFileInput = RegisterFileSchema.parse(request.body);

      const maxFileSize = BigInt(await this.configService.getValue("maxFileSize"));
      if (BigInt(input.size) > maxFileSize) {
        const maxSizeMB = Number(maxFileSize) / (1024 * 1024);
        return reply.status(400).send({
          error: `File size exceeds the maximum allowed size of ${maxSizeMB}MB`,
        });
      }

      const maxTotalStorage = BigInt(await this.configService.getValue("maxTotalStoragePerUser"));

      const userFiles = await prisma.file.findMany({
        where: { userId },
        select: { size: true },
      });

      const currentStorage = userFiles.reduce((acc, file) => acc + file.size, BigInt(0));

      if (currentStorage + BigInt(input.size) > maxTotalStorage) {
        const availableSpace = Number(maxTotalStorage - currentStorage) / (1024 * 1024);
        return reply.status(400).send({
          error: `Insufficient storage space. You have ${availableSpace.toFixed(2)}MB available`,
        });
      }

      const fileRecord = await prisma.file.create({
        data: {
          name: input.name,
          description: input.description,
          extension: input.extension,
          size: BigInt(input.size),
          objectName: input.objectName,
          userId,
        },
      });

      const fileResponse = {
        id: fileRecord.id,
        name: fileRecord.name,
        description: fileRecord.description,
        extension: fileRecord.extension,
        size: fileRecord.size.toString(),
        objectName: fileRecord.objectName,
        userId: fileRecord.userId,
        createdAt: fileRecord.createdAt,
        updatedAt: fileRecord.updatedAt,
      };

      return reply.status(201).send({
        file: fileResponse,
        message: "File registered successfully.",
      });
    } catch (error: any) {
      console.error("Error in registerFile:", error);
      return reply.status(400).send({ error: error.message });
    }
  }

  async checkFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({
          error: "Unauthorized: a valid token is required to access this resource.",
          code: "unauthorized",
        });
      }

      const input: CheckFileInput = CheckFileSchema.parse(request.body);

      const maxFileSize = BigInt(await this.configService.getValue("maxFileSize"));
      if (BigInt(input.size) > maxFileSize) {
        const maxSizeMB = Number(maxFileSize) / (1024 * 1024);
        return reply.status(400).send({
          code: "fileSizeExceeded",
          error: `File size exceeds the maximum allowed size of ${maxSizeMB}MB`,
          details: maxSizeMB.toString(),
        });
      }

      const maxTotalStorage = BigInt(await this.configService.getValue("maxTotalStoragePerUser"));

      const userFiles = await prisma.file.findMany({
        where: { userId },
        select: { size: true },
      });

      const currentStorage = userFiles.reduce((acc, file) => acc + file.size, BigInt(0));

      if (currentStorage + BigInt(input.size) > maxTotalStorage) {
        const availableSpace = Number(maxTotalStorage - currentStorage) / (1024 * 1024);
        return reply.status(400).send({
          error: `Insufficient storage space. You have ${availableSpace.toFixed(2)}MB available`,
          code: "insufficientStorage",
          details: availableSpace.toFixed(2),
        });
      }

      return reply.status(201).send({
        message: "File checks succeeded.",
      });
    } catch (error: any) {
      console.error("Error in checkFile:", error);
      return reply.status(400).send({ error: error.message });
    }
  }

  async getDownloadUrl(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { objectName: encodedObjectName } = request.params as {
        objectName: string;
      };
      const objectName = decodeURIComponent(encodedObjectName);

      if (!objectName) {
        return reply.status(400).send({ error: "The 'objectName' parameter is required." });
      }

      const fileRecord = await prisma.file.findFirst({ where: { objectName } });

      if (!fileRecord) {
        return reply.status(404).send({ error: "File not found." });
      }
      const fileName = fileRecord.name;
      const expires = 3600;
      const url = await this.fileService.getPresignedGetUrl(objectName, expires, fileName);
      return reply.send({ url, expiresIn: expires });
    } catch (error) {
      console.error("Error in getDownloadUrl:", error);
      return reply.status(500).send({ error: "Internal server error." });
    }
  }

  async listFiles(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;
      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const files = await prisma.file.findMany({
        where: { userId },
      });

      const filesResponse = files.map((file) => ({
        id: file.id,
        name: file.name,
        description: file.description,
        extension: file.extension,
        size: file.size.toString(),
        objectName: file.objectName,
        userId: file.userId,
        createdAt: file.createdAt,
        updatedAt: file.updatedAt,
      }));

      return reply.send({ files: filesResponse });
    } catch (error) {
      console.error("Error in listFiles:", error);
      return reply.status(500).send({ error: "Internal server error." });
    }
  }

  async deleteFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const { id } = request.params as { id: string };
      if (!id) {
        return reply.status(400).send({ error: "The 'id' parameter is required." });
      }

      const fileRecord = await prisma.file.findUnique({ where: { id } });
      if (!fileRecord) {
        return reply.status(404).send({ error: "File not found." });
      }

      const userId = (request as any).user?.userId;
      if (fileRecord.userId !== userId) {
        return reply.status(403).send({ error: "Access denied." });
      }

      await this.fileService.deleteObject(fileRecord.objectName);

      await prisma.file.delete({ where: { id } });

      return reply.send({ message: "File deleted successfully." });
    } catch (error) {
      console.error("Error in deleteFile:", error);
      return reply.status(500).send({ error: "Internal server error." });
    }
  }

  async updateFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const { id } = request.params as { id: string };
      const userId = (request as any).user?.userId;

      if (!userId) {
        return reply.status(401).send({
          error: "Unauthorized: a valid token is required to access this resource.",
        });
      }

      const updateData = UpdateFileSchema.parse(request.body);

      const fileRecord = await prisma.file.findUnique({ where: { id } });

      if (!fileRecord) {
        return reply.status(404).send({ error: "File not found." });
      }

      if (fileRecord.userId !== userId) {
        return reply.status(403).send({ error: "Access denied." });
      }

      const updatedFile = await prisma.file.update({
        where: { id },
        data: updateData,
      });

      const fileResponse = {
        id: updatedFile.id,
        name: updatedFile.name,
        description: updatedFile.description,
        extension: updatedFile.extension,
        size: updatedFile.size.toString(),
        objectName: updatedFile.objectName,
        userId: updatedFile.userId,
        createdAt: updatedFile.createdAt,
        updatedAt: updatedFile.updatedAt,
      };

      return reply.send({
        file: fileResponse,
        message: "File updated successfully.",
      });
    } catch (error: any) {
      console.error("Error in updateFile:", error);
      return reply.status(400).send({ error: error.message });
    }
  }
}
