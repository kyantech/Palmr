import { isS3Enabled } from "../../config/storage.config";
import { FilesystemStorageProvider } from "../../providers/filesystem-storage.provider";
import { S3StorageProvider } from "../../providers/s3-storage.provider";
import { prisma } from "../../shared/prisma";
import { StorageProvider } from "../../types/storage";

export class FolderService {
  private storageProvider: StorageProvider;

  constructor() {
    if (isS3Enabled) {
      this.storageProvider = new S3StorageProvider();
    } else {
      this.storageProvider = FilesystemStorageProvider.getInstance();
    }
  }

  async getPresignedPutUrl(objectName: string, expires: number): Promise<string> {
    try {
      return await this.storageProvider.getPresignedPutUrl(objectName, expires);
    } catch (err) {
      console.error("Erro no presignedPutObject:", err);
      throw err;
    }
  }

  async getPresignedGetUrl(objectName: string, expires: number, folderName?: string): Promise<string> {
    try {
      return await this.storageProvider.getPresignedGetUrl(objectName, expires, folderName);
    } catch (err) {
      console.error("Erro no presignedGetObject:", err);
      throw err;
    }
  }

  async deleteObject(objectName: string): Promise<void> {
    try {
      await this.storageProvider.deleteObject(objectName);
    } catch (err) {
      console.error("Erro no removeObject:", err);
      throw err;
    }
  }

  isFilesystemMode(): boolean {
    return !isS3Enabled;
  }

  async getAllFilesInFolder(folderId: string, userId: string, basePath: string = ""): Promise<any[]> {
    // Get all files directly in this folder
    const files = await prisma.file.findMany({
      where: { folderId, userId },
    });

    // Get all subfolders
    const subfolders = await prisma.folder.findMany({
      where: { parentId: folderId, userId },
      select: { id: true, name: true },
    });

    // Add relative path to files in this folder
    let allFiles = files.map((file: any) => ({
      ...file,
      relativePath: basePath + file.name,
    }));

    // Recursively get files from subfolders
    for (const subfolder of subfolders) {
      const subfolderPath = basePath + subfolder.name + "/";
      const subfolderFiles = await this.getAllFilesInFolder(subfolder.id, userId, subfolderPath);
      allFiles = [...allFiles, ...subfolderFiles];
    }

    return allFiles;
  }

  async calculateFolderSize(folderId: string, userId: string): Promise<bigint> {
    // Get all files directly in this folder
    const files = await prisma.file.findMany({
      where: { folderId, userId },
      select: { size: true },
    });

    // Get all subfolders
    const subfolders = await prisma.folder.findMany({
      where: { parentId: folderId, userId },
      select: { id: true },
    });

    // Sum up file sizes in this folder
    let totalSize = files.reduce((sum, file) => sum + file.size, BigInt(0));

    // Recursively calculate sizes of subfolders
    for (const subfolder of subfolders) {
      const subfolderSize = await this.calculateFolderSize(subfolder.id, userId);
      totalSize += subfolderSize;
    }

    return totalSize;
  }
}
