import { S3StorageProvider } from "../../providers/s3-storage.provider";
import { StorageProvider } from "../../types/storage";

export class FileService {
  private storageProvider: StorageProvider;

  constructor() {
    this.storageProvider = new S3StorageProvider();
  }

  async getPresignedPutUrl(objectName: string, expires: number): Promise<string> {
    try {
      return await this.storageProvider.getPresignedPutUrl(objectName, expires);
    } catch (err) {
      console.error("Erro no presignedPutObject:", err);
      throw err;
    }
  }

  async getPresignedGetUrl(objectName: string, expires: number, fileName?: string): Promise<string> {
    try {
      return await this.storageProvider.getPresignedGetUrl(objectName, expires, fileName);
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
}
