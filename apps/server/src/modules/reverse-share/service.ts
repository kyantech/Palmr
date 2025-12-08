import { PrismaClient } from "@prisma/client";

import { env } from "../../env";
import { EmailService } from "../email/service";
import { FileService } from "../file/service";
import { UserService } from "../user/service";
import {
  CreateReverseShareInput,
  ReverseShareResponseSchema,
  UpdateReverseShareInput,
  UploadToReverseShareInput,
} from "./dto";
import { ReverseShareRepository } from "./repository";

interface ReverseShareData {
  id: string;
  name: string | null;
  description: string | null;
  expiration: Date | null;
  maxFiles: number | null;
  maxFileSize: bigint | null;
  allowedFileTypes: string | null;
  password: string | null;
  pageLayout: string;
  isActive: boolean;
  nameFieldRequired: string;
  emailFieldRequired: string;
  createdAt: Date;
  updatedAt: Date;
  creatorId: string;
  files: any[];
  alias?: {
    id: string;
    alias: string;
    reverseShareId: string;
    createdAt: Date;
    updatedAt: Date;
  } | null;
}

const prisma = new PrismaClient();

export class ReverseShareService {
  private reverseShareRepository = new ReverseShareRepository();
  private fileService = new FileService();
  private emailService = new EmailService();
  private userService = new UserService();

  private uploadSessions = new Map<
    string,
    {
      reverseShareId: string;
      uploaderName: string;
      uploaderEmail?: string;
      files: string[];
      timeout: NodeJS.Timeout;
    }
  >();

  async createReverseShare(data: CreateReverseShareInput, creatorId: string) {
    const reverseShare = await this.reverseShareRepository.create(data, creatorId);
    return ReverseShareResponseSchema.parse(this.formatReverseShareResponse(reverseShare));
  }

  async listUserReverseShares(creatorId: string) {
    const reverseShares = await this.reverseShareRepository.findByCreatorId(creatorId);

    const formatted = reverseShares.map((reverseShare: ReverseShareData) =>
      ReverseShareResponseSchema.parse(this.formatReverseShareResponse(reverseShare))
    );

    return formatted;
  }

  async getReverseShareById(id: string, creatorId?: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (creatorId && reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to access this reverse share");
    }

    return ReverseShareResponseSchema.parse(this.formatReverseShareResponse(reverseShare));
  }

  async getReverseShareForUpload(id: string, password?: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    const currentFileCount = await this.reverseShareRepository.countFilesByReverseShareId(id);

    return {
      id: reverseShare.id,
      name: reverseShare.name,
      description: reverseShare.description,
      maxFiles: reverseShare.maxFiles,
      maxFileSize: reverseShare.maxFileSize ? Number(reverseShare.maxFileSize) : null,
      allowedFileTypes: reverseShare.allowedFileTypes,
      pageLayout: reverseShare.pageLayout,
      hasPassword: !!reverseShare.password,
      currentFileCount,
      nameFieldRequired: reverseShare.nameFieldRequired,
      emailFieldRequired: reverseShare.emailFieldRequired,
    };
  }

  async getReverseShareForUploadByAlias(alias: string, password?: string) {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    const currentFileCount = await this.reverseShareRepository.countFilesByReverseShareId(reverseShare.id);

    return {
      id: reverseShare.id,
      name: reverseShare.name,
      description: reverseShare.description,
      maxFiles: reverseShare.maxFiles,
      maxFileSize: reverseShare.maxFileSize ? Number(reverseShare.maxFileSize) : null,
      allowedFileTypes: reverseShare.allowedFileTypes,
      pageLayout: reverseShare.pageLayout,
      hasPassword: !!reverseShare.password,
      currentFileCount,
      nameFieldRequired: reverseShare.nameFieldRequired,
      emailFieldRequired: reverseShare.emailFieldRequired,
    };
  }

  async updateReverseShare(id: string, data: Partial<UpdateReverseShareInput>, creatorId: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to update this reverse share");
    }

    const updatedReverseShare = await this.reverseShareRepository.update(id, data);
    return ReverseShareResponseSchema.parse(this.formatReverseShareResponse(updatedReverseShare));
  }

  async deleteReverseShare(id: string, creatorId: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to delete this reverse share");
    }

    for (const file of reverseShare.files) {
      try {
        await this.fileService.deleteObject(file.objectName);
      } catch (error) {
        console.error(`Failed to delete file ${file.objectName}:`, error);
      }
    }

    const deletedReverseShare = await this.reverseShareRepository.delete(id);
    return ReverseShareResponseSchema.parse(this.formatReverseShareResponse(deletedReverseShare));
  }

  async getPresignedUrl(id: string, objectName: string, password?: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    const expires = parseInt(env.PRESIGNED_URL_EXPIRATION);

    // Import storage config to check if using internal or external S3
    const { isInternalStorage } = await import("../../config/storage.config.js");

    if (isInternalStorage) {
      // Internal storage: Use backend proxy for uploads (127.0.0.1 not accessible from client)
      // Note: This would need request context, but reverse-shares are typically used by external users
      // For now, we'll use presigned URLs and handle the error on the client side
      const url = await this.fileService.getPresignedPutUrl(objectName, expires);
      return { url, expiresIn: expires };
    } else {
      // External S3: Use presigned URLs directly (more efficient)
      const url = await this.fileService.getPresignedPutUrl(objectName, expires);
      return { url, expiresIn: expires };
    }
  }

  async getPresignedUrlByAlias(alias: string, objectName: string, password?: string) {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    const expires = parseInt(env.PRESIGNED_URL_EXPIRATION);

    // Import storage config to check if using internal or external S3
    const { isInternalStorage } = await import("../../config/storage.config.js");

    if (isInternalStorage) {
      // Internal storage: Use backend proxy for uploads (127.0.0.1 not accessible from client)
      // Note: This would need request context, but reverse-shares are typically used by external users
      // For now, we'll use presigned URLs and handle the error on the client side
      const url = await this.fileService.getPresignedPutUrl(objectName, expires);
      return { url, expiresIn: expires };
    } else {
      // External S3: Use presigned URLs directly (more efficient)
      const url = await this.fileService.getPresignedPutUrl(objectName, expires);
      return { url, expiresIn: expires };
    }
  }

  async registerFileUpload(reverseShareId: string, fileData: UploadToReverseShareInput, password?: string) {
    const reverseShare = await this.reverseShareRepository.findById(reverseShareId);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    if (reverseShare.maxFiles) {
      const currentFileCount = await this.reverseShareRepository.countFilesByReverseShareId(reverseShareId);
      if (currentFileCount >= reverseShare.maxFiles) {
        throw new Error("Maximum number of files reached");
      }
    }

    if (reverseShare.maxFileSize && BigInt(fileData.size) > reverseShare.maxFileSize) {
      throw new Error("File size exceeds limit");
    }

    if (reverseShare.allowedFileTypes) {
      const allowedTypes = reverseShare.allowedFileTypes.split(",").map((type) => type.trim().toLowerCase());
      if (!allowedTypes.includes(fileData.extension.toLowerCase())) {
        throw new Error("File type not allowed");
      }
    }

    const file = await this.reverseShareRepository.createFile(reverseShareId, {
      ...fileData,
      size: BigInt(fileData.size),
    });

    this.addFileToUploadSession(reverseShare, fileData);

    return this.formatFileResponse(file);
  }

  async registerFileUploadByAlias(alias: string, fileData: UploadToReverseShareInput, password?: string) {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    if (reverseShare.maxFiles) {
      const currentFileCount = await this.reverseShareRepository.countFilesByReverseShareId(reverseShare.id);
      if (currentFileCount >= reverseShare.maxFiles) {
        throw new Error("Maximum number of files reached");
      }
    }

    if (reverseShare.maxFileSize && BigInt(fileData.size) > reverseShare.maxFileSize) {
      throw new Error("File size exceeds limit");
    }

    if (reverseShare.allowedFileTypes) {
      const allowedTypes = reverseShare.allowedFileTypes.split(",").map((type) => type.trim().toLowerCase());
      if (!allowedTypes.includes(fileData.extension.toLowerCase())) {
        throw new Error("File type not allowed");
      }
    }

    const file = await this.reverseShareRepository.createFile(reverseShare.id, {
      ...fileData,
      size: BigInt(fileData.size),
    });

    this.addFileToUploadSession(reverseShare, fileData);

    return this.formatFileResponse(file);
  }

  async getFileInfo(fileId: string, creatorId: string) {
    const file = await this.reverseShareRepository.findFileById(fileId);
    if (!file) {
      throw new Error("File not found");
    }

    if (file.reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to access this file");
    }

    return {
      id: file.id,
      name: file.name,
      size: file.size,
      objectName: file.objectName,
      extension: file.extension,
    };
  }

  async downloadReverseShareFile(
    fileId: string,
    creatorId: string,
    requestContext?: { protocol: string; host: string }
  ) {
    const file = await this.reverseShareRepository.findFileById(fileId);
    if (!file) {
      throw new Error("File not found");
    }

    if (file.reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to download this file");
    }

    const fileName = file.name;
    const expires = parseInt(env.PRESIGNED_URL_EXPIRATION);

    // Import storage config to check if using internal or external S3
    const { isInternalStorage } = await import("../../config/storage.config.js");

    if (isInternalStorage) {
      // Internal storage: Use frontend proxy (much simpler!)
      const url = `/api/files/download?objectName=${encodeURIComponent(file.objectName)}`;
      return { url, expiresIn: expires };
    } else {
      // External S3: Use presigned URLs directly (more efficient, no backend proxy)
      const url = await this.fileService.getPresignedGetUrl(file.objectName, expires, fileName);
      return { url, expiresIn: expires };
    }
  }

  async deleteReverseShareFile(fileId: string, creatorId: string) {
    const file = await this.reverseShareRepository.findFileById(fileId);
    if (!file) {
      throw new Error("File not found");
    }

    if (file.reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to delete this file");
    }

    await this.fileService.deleteObject(file.objectName);

    const deletedFile = await this.reverseShareRepository.deleteFile(fileId);
    return this.formatFileResponse(deletedFile);
  }

  async checkPassword(id: string, password: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.password) {
      return { valid: true };
    }

    const isValid = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
    return { valid: isValid };
  }

  async updatePassword(id: string, password: string | null, creatorId: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to update this reverse share");
    }

    const updatedReverseShare = await this.reverseShareRepository.update(id, { password });
    return ReverseShareResponseSchema.parse(this.formatReverseShareResponse(updatedReverseShare));
  }

  async activateReverseShare(id: string, creatorId: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to activate this reverse share");
    }

    const updatedReverseShare = await this.reverseShareRepository.update(id, { isActive: true });
    return ReverseShareResponseSchema.parse(this.formatReverseShareResponse(updatedReverseShare));
  }

  async deactivateReverseShare(id: string, creatorId: string) {
    const reverseShare = await this.reverseShareRepository.findById(id);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to deactivate this reverse share");
    }

    const updatedReverseShare = await this.reverseShareRepository.update(id, { isActive: false });
    return ReverseShareResponseSchema.parse(this.formatReverseShareResponse(updatedReverseShare));
  }

  async createOrUpdateAlias(reverseShareId: string, alias: string, userId: string) {
    const reverseShare = await this.reverseShareRepository.findById(reverseShareId);

    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (reverseShare.creatorId !== userId) {
      throw new Error("Unauthorized to update this reverse share");
    }

    const existingAlias = await prisma.reverseShareAlias.findUnique({
      where: { alias },
    });

    if (existingAlias && existingAlias.reverseShareId !== reverseShareId) {
      throw new Error("Alias already in use");
    }

    const reverseShareAlias = await prisma.reverseShareAlias.upsert({
      where: { reverseShareId },
      create: { reverseShareId, alias },
      update: { alias },
    });

    return {
      ...reverseShareAlias,
      createdAt: reverseShareAlias.createdAt.toISOString(),
      updatedAt: reverseShareAlias.updatedAt.toISOString(),
    };
  }

  async updateReverseShareFile(
    fileId: string,
    data: { name?: string; description?: string | null },
    creatorId: string
  ) {
    const file = await this.reverseShareRepository.findFileById(fileId);
    if (!file) {
      throw new Error("File not found");
    }

    if (file.reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to edit this file");
    }

    const updateData = { ...data };
    if (data.name) {
      const originalExtension = file.extension;
      const nameWithoutExtension = data.name.replace(/\.[^/.]+$/, "");
      const extensionWithDot = originalExtension.startsWith(".") ? originalExtension : `.${originalExtension}`;
      updateData.name = `${nameWithoutExtension}${extensionWithDot}`;
    }

    const updatedFile = await this.reverseShareRepository.updateFile(fileId, updateData);
    return this.formatFileResponse(updatedFile);
  }

  async copyReverseShareFileToUserFiles(fileId: string, creatorId: string) {
    const file = await this.reverseShareRepository.findFileById(fileId);
    if (!file) {
      throw new Error("File not found");
    }

    if (file.reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to copy this file");
    }

    const { prisma } = await import("../../shared/prisma.js");
    const { ConfigService } = await import("../config/service.js");
    const configService = new ConfigService();

    const maxFileSize = BigInt(await configService.getValue("maxFileSize"));
    if (file.size > maxFileSize) {
      const maxSizeMB = Number(maxFileSize) / (1024 * 1024);
      throw new Error(`File size exceeds the maximum allowed size of ${maxSizeMB}MB`);
    }

    const maxTotalStorage = BigInt(await configService.getValue("maxTotalStoragePerUser"));

    const userFiles = await prisma.file.findMany({
      where: { userId: creatorId },
      select: { size: true },
    });

    const currentStorage = userFiles.reduce((acc: bigint, userFile: any) => acc + userFile.size, BigInt(0));

    if (currentStorage + file.size > maxTotalStorage) {
      const availableSpace = Number(maxTotalStorage - currentStorage) / (1024 * 1024);
      throw new Error(`Insufficient storage space. You have ${availableSpace.toFixed(2)}MB available`);
    }

    const newObjectName = `${creatorId}/${Date.now()}-${file.name}`;

    // Copy file using S3 presigned URLs
    const fileSizeMB = Number(file.size) / (1024 * 1024);
    const needsStreaming = fileSizeMB > 100;

    const downloadUrl = await this.fileService.getPresignedGetUrl(file.objectName, 300);
    const uploadUrl = await this.fileService.getPresignedPutUrl(newObjectName, 300);

    let retries = 0;
    const maxRetries = 3;
    let success = false;

    while (retries < maxRetries && !success) {
      try {
        const response = await fetch(downloadUrl, {
          signal: AbortSignal.timeout(600000), // 10 minutes timeout
        });

        if (!response.ok) {
          throw new Error(`Failed to download file: ${response.statusText}`);
        }

        if (!response.body) {
          throw new Error("No response body received");
        }

        const uploadOptions: any = {
          method: "PUT",
          body: response.body,
          duplex: "half",
          headers: {
            "Content-Type": "application/octet-stream",
            "Content-Length": file.size.toString(),
          },
          signal: AbortSignal.timeout(9600000), // 160 minutes timeout
        };

        const uploadResponse = await fetch(uploadUrl, uploadOptions);

        if (!uploadResponse.ok) {
          const errorText = await uploadResponse.text();
          throw new Error(`Failed to upload file: ${uploadResponse.statusText} - ${errorText}`);
        }

        success = true;
      } catch (error: any) {
        retries++;

        if (retries >= maxRetries) {
          throw new Error(`Failed to copy file after ${maxRetries} attempts: ${error.message}`);
        }

        const delay = Math.min(1000 * Math.pow(2, retries - 1), 10000);
        await new Promise((resolve) => setTimeout(resolve, delay));
      }
    }

    const newFileRecord = await prisma.file.create({
      data: {
        name: file.name,
        description: file.description || `Copied from: ${file.reverseShare.name || "Unnamed"}`,
        extension: file.extension,
        size: file.size,
        objectName: newObjectName,
        userId: creatorId,
      },
    });

    return {
      id: newFileRecord.id,
      name: newFileRecord.name,
      description: newFileRecord.description,
      extension: newFileRecord.extension,
      size: newFileRecord.size.toString(),
      objectName: newFileRecord.objectName,
      userId: newFileRecord.userId,
      createdAt: newFileRecord.createdAt.toISOString(),
      updatedAt: newFileRecord.updatedAt.toISOString(),
    };
  }

  private generateSessionKey(reverseShareId: string, uploaderIdentifier: string): string {
    return `${reverseShareId}-${uploaderIdentifier}`;
  }

  private async sendBatchFileUploadNotification(reverseShare: any, uploaderName: string, fileNames: string[]) {
    try {
      const creator = await this.userService.getUserById(reverseShare.creatorId);
      const reverseShareName = reverseShare.name || "Unnamed Reverse Share";
      const fileCount = fileNames.length;
      const fileList = fileNames.join(", ");

      await this.emailService.sendReverseShareBatchFileNotification(
        creator.email,
        reverseShareName,
        fileCount,
        fileList,
        uploaderName
      );
    } catch (error) {
      console.error("Failed to send reverse share batch file notification:", error);
    }
  }

  private addFileToUploadSession(reverseShare: any, fileData: UploadToReverseShareInput) {
    const uploaderIdentifier = fileData.uploaderEmail || fileData.uploaderName || "anonymous";
    const sessionKey = this.generateSessionKey(reverseShare.id, uploaderIdentifier);
    const uploaderName = fileData.uploaderName || "Someone";

    const existingSession = this.uploadSessions.get(sessionKey);
    if (existingSession) {
      clearTimeout(existingSession.timeout);
      existingSession.files.push(fileData.name);
    } else {
      this.uploadSessions.set(sessionKey, {
        reverseShareId: reverseShare.id,
        uploaderName,
        uploaderEmail: fileData.uploaderEmail,
        files: [fileData.name],
        timeout: null as any,
      });
    }

    const session = this.uploadSessions.get(sessionKey)!;
    session.timeout = setTimeout(async () => {
      await this.sendBatchFileUploadNotification(reverseShare, session.uploaderName, session.files);
      this.uploadSessions.delete(sessionKey);
    }, 5000);
  }

  private formatReverseShareResponse(reverseShare: ReverseShareData) {
    const result = {
      id: reverseShare.id,
      name: reverseShare.name,
      description: reverseShare.description,
      expiration: reverseShare.expiration?.toISOString() || null,
      maxFiles: reverseShare.maxFiles,
      maxFileSize: reverseShare.maxFileSize ? Number(reverseShare.maxFileSize) : null,
      allowedFileTypes: reverseShare.allowedFileTypes,
      pageLayout: reverseShare.pageLayout,
      isActive: reverseShare.isActive,
      hasPassword: !!reverseShare.password,
      createdAt: reverseShare.createdAt.toISOString(),
      updatedAt: reverseShare.updatedAt.toISOString(),
      creatorId: reverseShare.creatorId,
      files: (reverseShare.files || []).map((file: any) => ({
        id: file.id,
        name: file.name,
        description: file.description,
        extension: file.extension,
        size: file.size.toString(),
        objectName: file.objectName,
        uploaderEmail: file.uploaderEmail,
        uploaderName: file.uploaderName,
        reverseShareId: file.reverseShareId,
        createdAt: file.createdAt.toISOString(),
        updatedAt: file.updatedAt.toISOString(),
      })),
      alias: reverseShare.alias
        ? {
            id: reverseShare.alias.id,
            alias: reverseShare.alias.alias,
            reverseShareId: reverseShare.alias.reverseShareId,
            createdAt: reverseShare.alias.createdAt.toISOString(),
            updatedAt: reverseShare.alias.updatedAt.toISOString(),
          }
        : null,
      nameFieldRequired: reverseShare.nameFieldRequired,
      emailFieldRequired: reverseShare.emailFieldRequired,
    };

    return result;
  }

  // Multipart upload methods for reverse shares
  async createMultipartUploadByAlias(
    alias: string,
    filename: string,
    extension: string,
    password?: string
  ): Promise<{ uploadId: string; objectName: string }> {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    // Generate unique object name
    const objectName = `reverse-shares/${alias}/${Date.now()}-${Math.random().toString(36).substring(7)}-${filename}.${extension}`;

    const uploadId = await this.fileService.createMultipartUpload(objectName);

    return {
      uploadId,
      objectName,
    };
  }

  async getMultipartPartUrlByAlias(
    alias: string,
    uploadId: string,
    objectName: string,
    partNumber: number,
    password?: string
  ): Promise<{ url: string }> {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    const expires = parseInt(env.PRESIGNED_URL_EXPIRATION);
    const url = await this.fileService.getPresignedPartUrl(objectName, uploadId, partNumber, expires);

    return { url };
  }

  async completeMultipartUploadByAlias(
    alias: string,
    uploadId: string,
    objectName: string,
    parts: Array<{ PartNumber: number; ETag: string }>,
    password?: string
  ): Promise<{ message: string; objectName: string }> {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    await this.fileService.completeMultipartUpload(objectName, uploadId, parts);

    return {
      message: "Multipart upload completed successfully",
      objectName,
    };
  }

  async abortMultipartUploadByAlias(
    alias: string,
    uploadId: string,
    objectName: string,
    password?: string
  ): Promise<{ message: string }> {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    if (!reverseShare.isActive) {
      throw new Error("Reverse share is inactive");
    }

    if (reverseShare.expiration && new Date(reverseShare.expiration) < new Date()) {
      throw new Error("Reverse share has expired");
    }

    if (reverseShare.password) {
      if (!password) {
        throw new Error("Password required");
      }
      const isValidPassword = await this.reverseShareRepository.comparePassword(password, reverseShare.password);
      if (!isValidPassword) {
        throw new Error("Invalid password");
      }
    }

    await this.fileService.abortMultipartUpload(objectName, uploadId);

    return {
      message: "Multipart upload aborted successfully",
    };
  }

  private formatFileResponse(file: any) {
    return {
      id: file.id,
      name: file.name,
      description: file.description,
      extension: file.extension,
      size: file.size.toString(),
      objectName: file.objectName,
      uploaderEmail: file.uploaderEmail,
      uploaderName: file.uploaderName,
      reverseShareId: file.reverseShareId,
      createdAt: file.createdAt.toISOString(),
      updatedAt: file.updatedAt.toISOString(),
    };
  }

  async getReverseShareMetadataByAlias(alias: string) {
    const reverseShare = await this.reverseShareRepository.findByAlias(alias);
    if (!reverseShare) {
      throw new Error("Reverse share not found");
    }

    // Check if reverse share is expired
    const isExpired = reverseShare.expiration ? new Date(reverseShare.expiration) < new Date() : false;

    // Check if inactive
    const isInactive = !reverseShare.isActive;

    const totalFiles = reverseShare.files?.length || 0;
    const hasPassword = !!reverseShare.password;

    return {
      name: reverseShare.name,
      description: reverseShare.description,
      totalFiles,
      hasPassword,
      isExpired,
      isInactive,
      maxFiles: reverseShare.maxFiles,
    };
  }
}
