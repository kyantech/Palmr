export interface StorageProvider {
  getPresignedPutUrl(objectName: string, expires: number): Promise<string>;
  getPresignedGetUrl(objectName: string, expires: number, fileName?: string): Promise<string>;
  deleteObject(objectName: string): Promise<void>;
  fileExists(objectName: string): Promise<boolean>;
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
