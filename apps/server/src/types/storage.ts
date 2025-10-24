export interface StorageProvider {
  getPresignedPutUrl(objectName: string, expires: number): Promise<string>;
  getPresignedGetUrl(objectName: string, expires: number, fileName?: string): Promise<string>;
  deleteObject(objectName: string): Promise<void>;
  fileExists(objectName: string): Promise<boolean>;
  getObjectStream(objectName: string): Promise<NodeJS.ReadableStream>;

  // Multipart upload methods
  createMultipartUpload(objectName: string): Promise<string>;
  getPresignedPartUrl(objectName: string, uploadId: string, partNumber: number, expires: number): Promise<string>;
  completeMultipartUpload(
    objectName: string,
    uploadId: string,
    parts: Array<{ PartNumber: number; ETag: string }>
  ): Promise<void>;
  abortMultipartUpload(objectName: string, uploadId: string): Promise<void>;
}

export interface StorageConfig {
  endpoint: string;
  port?: number;
  useSSL: boolean;
  accessKey: string;
  secretKey: string;
  region: string;
  bucketName: string;
  forcePathStyle?: boolean;
}
