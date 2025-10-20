import bcrypt from "bcryptjs";
import { FastifyReply, FastifyRequest } from "fastify";

import { env } from "../../env";
import { prisma } from "../../shared/prisma";
import { generateUniqueFileName, parseFileName } from "../../utils/file-name-generator";
import { ConfigService } from "../config/service";
import {
  CheckFileInput,
  CheckFileSchema,
  ListFilesInput,
  ListFilesSchema,
  MoveFileInput,
  MoveFileSchema,
  RegisterFileInput,
  RegisterFileSchema,
  UpdateFileInput,
  UpdateFileSchema,
} from "./dto";
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
      const expires = parseInt(env.PRESIGNED_URL_EXPIRATION);

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

      if (input.folderId) {
        const folder = await prisma.folder.findFirst({
          where: { id: input.folderId, userId },
        });
        if (!folder) {
          return reply.status(400).send({ error: "Folder not found or access denied." });
        }
      }

      // Parse the filename and generate a unique name if there's a duplicate
      const { baseName, extension } = parseFileName(input.name);
      const uniqueName = await generateUniqueFileName(baseName, extension, userId, input.folderId);

      const fileRecord = await prisma.file.create({
        data: {
          name: uniqueName,
          description: input.description,
          extension: input.extension,
          size: BigInt(input.size),
          objectName: input.objectName,
          userId,
          folderId: input.folderId,
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
        folderId: fileRecord.folderId,
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

      // Check for duplicate filename and provide the suggested unique name
      const { baseName, extension } = parseFileName(input.name);
      const uniqueName = await generateUniqueFileName(baseName, extension, userId, input.folderId);

      // Include suggestedName in response if the name was changed
      const response: any = {
        message: "File checks succeeded.",
      };

      if (uniqueName !== input.name) {
        response.suggestedName = uniqueName;
      }

      return reply.status(201).send(response);
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
      const { password } = request.query as { password?: string };

      if (!objectName) {
        return reply.status(400).send({ error: "The 'objectName' parameter is required." });
      }

      const fileRecord = await prisma.file.findFirst({ where: { objectName } });

      if (!fileRecord) {
        return reply.status(404).send({ error: "File not found." });
      }

      let hasAccess = false;

      console.log("Requested file with password " + password);

      const shares = await prisma.share.findMany({
        where: {
          files: {
            some: {
              id: fileRecord.id,
            },
          },
        },
        include: {
          security: true,
        },
      });

      for (const share of shares) {
        if (!share.security.password) {
          hasAccess = true;
          break;
        } else if (password) {
          const isPasswordValid = await bcrypt.compare(password, share.security.password);
          if (isPasswordValid) {
            hasAccess = true;
            break;
          }
        }
      }

      if (!hasAccess) {
        try {
          await request.jwtVerify();
          const userId = (request as any).user?.userId;
          if (userId && fileRecord.userId === userId) {
            hasAccess = true;
          }
        } catch (err) {}
      }

      if (!hasAccess) {
        return reply.status(401).send({ error: "Unauthorized access to file." });
      }

      const fileName = fileRecord.name;
      const expires = parseInt(env.PRESIGNED_URL_EXPIRATION);
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

      const input: ListFilesInput = ListFilesSchema.parse(request.query);
      const { folderId, recursive: recursiveStr } = input;
      const recursive = recursiveStr === "false" ? false : true;

      let files: any[];

      let targetFolderId: string | null;
      if (folderId === "null" || folderId === "" || !folderId) {
        targetFolderId = null; // Root folder
      } else {
        targetFolderId = folderId;
      }

      if (recursive) {
        if (targetFolderId === null) {
          files = await this.getAllUserFilesRecursively(userId);
        } else {
          const { FolderService } = await import("../folder/service.js");
          const folderService = new FolderService();
          files = await folderService.getAllFilesInFolder(targetFolderId, userId);
        }
      } else {
        files = await prisma.file.findMany({
          where: { userId, folderId: targetFolderId },
        });
      }

      const filesResponse = files.map((file: any) => ({
        id: file.id,
        name: file.name,
        description: file.description,
        extension: file.extension,
        size: typeof file.size === "bigint" ? file.size.toString() : file.size,
        objectName: file.objectName,
        userId: file.userId,
        folderId: file.folderId,
        relativePath: file.relativePath || null,
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
        folderId: updatedFile.folderId,
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

  async moveFile(request: FastifyRequest, reply: FastifyReply) {
    try {
      await request.jwtVerify();
      const userId = (request as any).user?.userId;

      if (!userId) {
        return reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
      }

      const { id } = request.params as { id: string };
      const input: MoveFileInput = MoveFileSchema.parse(request.body);

      const existingFile = await prisma.file.findFirst({
        where: { id, userId },
      });

      if (!existingFile) {
        return reply.status(404).send({ error: "File not found." });
      }

      if (input.folderId) {
        const targetFolder = await prisma.folder.findFirst({
          where: { id: input.folderId, userId },
        });
        if (!targetFolder) {
          return reply.status(400).send({ error: "Target folder not found." });
        }
      }

      const updatedFile = await prisma.file.update({
        where: { id },
        data: { folderId: input.folderId },
      });

      const fileResponse = {
        id: updatedFile.id,
        name: updatedFile.name,
        description: updatedFile.description,
        extension: updatedFile.extension,
        size: updatedFile.size.toString(),
        objectName: updatedFile.objectName,
        userId: updatedFile.userId,
        folderId: updatedFile.folderId,
        createdAt: updatedFile.createdAt,
        updatedAt: updatedFile.updatedAt,
      };

      return reply.send({
        file: fileResponse,
        message: "File moved successfully.",
      });
    } catch (error: any) {
      console.error("Error moving file:", error);
      return reply.status(400).send({ error: error.message });
    }
  }

  private async getAllUserFilesRecursively(userId: string): Promise<any[]> {
    const rootFiles = await prisma.file.findMany({
      where: { userId, folderId: null },
    });

    const rootFolders = await prisma.folder.findMany({
      where: { userId, parentId: null },
      select: { id: true },
    });

    let allFiles = [...rootFiles];

    if (rootFolders.length > 0) {
      const { FolderService } = await import("../folder/service.js");
      const folderService = new FolderService();

      for (const folder of rootFolders) {
        const folderFiles = await folderService.getAllFilesInFolder(folder.id, userId);
        allFiles = [...allFiles, ...folderFiles];
      }
    }

    return allFiles;
  }
}
