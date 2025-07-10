import { PrismaClient } from "@prisma/client";

import { FileService } from "../file/service";
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

    const expires = 3600; // 1 hour
    const url = await this.fileService.getPresignedPutUrl(objectName, expires);

    return { url, expiresIn: expires };
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

    const expires = 3600; // 1 hour
    const url = await this.fileService.getPresignedPutUrl(objectName, expires);

    return { url, expiresIn: expires };
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

    return this.formatFileResponse(file);
  }

  async downloadReverseShareFile(fileId: string, creatorId: string) {
    const file = await this.reverseShareRepository.findFileById(fileId);
    if (!file) {
      throw new Error("File not found");
    }

    if (file.reverseShare.creatorId !== creatorId) {
      throw new Error("Unauthorized to download this file");
    }

    const fileName = file.name;
    const expires = 3600; // 1 hour
    const url = await this.fileService.getPresignedGetUrl(file.objectName, expires, fileName);
    return { url, expiresIn: expires };
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

    if (this.fileService.isFilesystemMode()) {
      const { FilesystemStorageProvider } = await import("../../providers/filesystem-storage.provider.js");
      const provider = FilesystemStorageProvider.getInstance();

      // Use streaming copy for filesystem mode
      const sourcePath = provider.getFilePath(file.objectName);
      const fs = await import("fs");
      const { pipeline } = await import("stream/promises");

      const sourceStream = fs.createReadStream(sourcePath);
      const decryptStream = provider.createDecryptStream();

      // Create a passthrough stream to get the decrypted content
      const { PassThrough } = await import("stream");
      const passThrough = new PassThrough();

      // First, decrypt the source file into the passthrough stream
      await pipeline(sourceStream, decryptStream, passThrough);

      // Then upload the decrypted content
      await provider.uploadFileFromStream(newObjectName, passThrough);
    } else {
      const downloadUrl = await this.fileService.getPresignedGetUrl(file.objectName, 300);
      const uploadUrl = await this.fileService.getPresignedPutUrl(newObjectName, 300);

      const response = await fetch(downloadUrl);
      if (!response.ok) {
        throw new Error(`Failed to download file: ${response.statusText}`);
      }

      if (!response.body) {
        throw new Error("No response body received");
      }

      const uploadResponse = await fetch(uploadUrl, {
        method: "PUT",
        body: response.body,
        headers: {
          "Content-Type": "application/octet-stream",
        },
      });

      if (!uploadResponse.ok) {
        throw new Error(`Failed to upload file: ${uploadResponse.statusText}`);
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
}
