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

  // Multipart upload methods
  async createMultipartUpload(objectName: string): Promise<string> {
    return await this.storageProvider.createMultipartUpload(objectName);
  }

  async getPresignedPartUrl(
    objectName: string,
    uploadId: string,
    partNumber: number,
    expires: number = 3600
  ): Promise<string> {
    return await this.storageProvider.getPresignedPartUrl(objectName, uploadId, partNumber, expires);
  }

  async completeMultipartUpload(
    objectName: string,
    uploadId: string,
    parts: Array<{ PartNumber: number; ETag: string }>
  ): Promise<void> {
    await this.storageProvider.completeMultipartUpload(objectName, uploadId, parts);
  }

  async abortMultipartUpload(objectName: string, uploadId: string): Promise<void> {
    await this.storageProvider.abortMultipartUpload(objectName, uploadId);
  }
}
