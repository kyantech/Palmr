import { S3StorageProvider } from "../../providers/s3-storage.provider";
import { StorageProvider } from "../../types/storage";

export class FileService {
  private storageProvider: StorageProvider;

  constructor() {
    // Always use S3 (Garage internal or external S3)
    this.storageProvider = new S3StorageProvider();
  }

  async getPresignedPutUrl(objectName: string, expires: number = 3600): Promise<string> {
    return await this.storageProvider.getPresignedPutUrl(objectName, expires);
  }

  async getPresignedGetUrl(objectName: string, expires: number = 3600, fileName?: string): Promise<string> {
    return await this.storageProvider.getPresignedGetUrl(objectName, expires, fileName);
  }

  async deleteObject(objectName: string): Promise<void> {
    try {
      await this.storageProvider.deleteObject(objectName);
    } catch (err) {
      console.error("Erro no removeObject:", err);
      throw err;
    }
  }

  async getObjectStream(objectName: string): Promise<NodeJS.ReadableStream> {
    try {
      return await this.storageProvider.getObjectStream(objectName);
    } catch (err) {
      console.error("Error getting object stream:", err);
      throw err;
    }
  }
}
