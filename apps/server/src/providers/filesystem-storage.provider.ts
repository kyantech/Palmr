import * as crypto from "crypto";
import * as fsSync from "fs";
import * as fs from "fs/promises";
import * as path from "path";
import { Transform } from "stream";
import { pipeline } from "stream/promises";

import { directoriesConfig, getTempFilePath } from "../config/directories.config";
import { env } from "../env";
import { StorageProvider } from "../types/storage";
import { IS_RUNNING_IN_CONTAINER } from "../utils/container-detection";

export class FilesystemStorageProvider implements StorageProvider {
  private static instance: FilesystemStorageProvider;
  private uploadsDir: string;
  private encryptionKey = env.ENCRYPTION_KEY;
  private uploadTokens = new Map<string, { objectName: string; expiresAt: number }>();
  private downloadTokens = new Map<string, { objectName: string; expiresAt: number; fileName?: string }>();

  private constructor() {
    this.uploadsDir = directoriesConfig.uploads;

    this.ensureUploadsDir();
    setInterval(() => this.cleanExpiredTokens(), 5 * 60 * 1000);
    setInterval(() => this.cleanupEmptyTempDirs(), 10 * 60 * 1000); // Every 10 minutes
  }

  public static getInstance(): FilesystemStorageProvider {
    if (!FilesystemStorageProvider.instance) {
      FilesystemStorageProvider.instance = new FilesystemStorageProvider();
    }
    return FilesystemStorageProvider.instance;
  }

  private async ensureUploadsDir(): Promise<void> {
    try {
      await fs.access(this.uploadsDir);
    } catch {
      await fs.mkdir(this.uploadsDir, { recursive: true });
    }
  }

  private cleanExpiredTokens(): void {
    const now = Date.now();

    for (const [token, data] of this.uploadTokens.entries()) {
      if (now > data.expiresAt) {
        this.uploadTokens.delete(token);
      }
    }

    for (const [token, data] of this.downloadTokens.entries()) {
      if (now > data.expiresAt) {
        this.downloadTokens.delete(token);
      }
    }
  }

  public getFilePath(objectName: string): string {
    const sanitizedName = objectName.replace(/[^a-zA-Z0-9\-_./]/g, "_");
    return path.join(this.uploadsDir, sanitizedName);
  }

  private createEncryptionKey(): Buffer {
    return crypto.scryptSync(this.encryptionKey, "salt", 32);
  }

  public createEncryptStream(): Transform {
    const key = this.createEncryptionKey();
    const iv = crypto.randomBytes(16);
    const cipher = crypto.createCipheriv("aes-256-cbc", key, iv);

    let isFirstChunk = true;

    return new Transform({
      transform(chunk, encoding, callback) {
        try {
          if (isFirstChunk) {
            this.push(iv);
            isFirstChunk = false;
          }

          const encrypted = cipher.update(chunk);
          this.push(encrypted);
          callback();
        } catch (error) {
          callback(error as Error);
        }
      },

      flush(callback) {
        try {
          const final = cipher.final();
          this.push(final);
          callback();
        } catch (error) {
          callback(error as Error);
        }
      },
    });
  }

  public createDecryptStream(): Transform {
    const key = this.createEncryptionKey();
    let iv: Buffer | null = null;
    let decipher: crypto.Decipher | null = null;
    let ivBuffer = Buffer.alloc(0);

    return new Transform({
      transform(chunk, encoding, callback) {
        try {
          if (!iv) {
            ivBuffer = Buffer.concat([ivBuffer, chunk]);

            if (ivBuffer.length >= 16) {
              iv = ivBuffer.slice(0, 16);
              decipher = crypto.createDecipheriv("aes-256-cbc", key, iv);
              const remainingData = ivBuffer.slice(16);
              if (remainingData.length > 0) {
                const decrypted = decipher.update(remainingData);
                this.push(decrypted);
              }
            }
            callback();
            return;
          }

          if (decipher) {
            const decrypted = decipher.update(chunk);
            this.push(decrypted);
          }
          callback();
        } catch (error) {
          callback(error as Error);
        }
      },

      flush(callback) {
        try {
          if (decipher) {
            const final = decipher.final();
            this.push(final);
          }
          callback();
        } catch (error) {
          callback(error as Error);
        }
      },
    });
  }

  async getPresignedPutUrl(objectName: string, expires: number): Promise<string> {
    const token = crypto.randomBytes(32).toString("hex");
    const expiresAt = Date.now() + expires * 1000;

    this.uploadTokens.set(token, { objectName, expiresAt });

    return `/api/filesystem/upload/${token}`;
  }

  async getPresignedGetUrl(objectName: string, expires: number, fileName?: string): Promise<string> {
    const token = crypto.randomBytes(32).toString("hex");
    const expiresAt = Date.now() + expires * 1000;

    this.downloadTokens.set(token, { objectName, expiresAt, fileName });

    return `/api/filesystem/download/${token}`;
  }

  async deleteObject(objectName: string): Promise<void> {
    const filePath = this.getFilePath(objectName);
    try {
      await fs.unlink(filePath);
    } catch (error: any) {
      if (error.code !== "ENOENT") {
        throw error;
      }
    }
  }

  async uploadFile(objectName: string, buffer: Buffer): Promise<void> {
    // For backward compatibility, convert buffer to stream and use streaming upload
    const filePath = this.getFilePath(objectName);
    const dir = path.dirname(filePath);

    await fs.mkdir(dir, { recursive: true });

    const { Readable } = await import("stream");
    const readable = Readable.from(buffer);

    await this.uploadFileFromStream(objectName, readable);
  }

  async uploadFileFromStream(objectName: string, inputStream: NodeJS.ReadableStream): Promise<void> {
    const filePath = this.getFilePath(objectName);
    const dir = path.dirname(filePath);

    await fs.mkdir(dir, { recursive: true });

    // Use the new temp file system for better organization
    const tempPath = getTempFilePath(objectName);
    const tempDir = path.dirname(tempPath);

    await fs.mkdir(tempDir, { recursive: true });

    const writeStream = fsSync.createWriteStream(tempPath);
    const encryptStream = this.createEncryptStream();

    try {
      await pipeline(inputStream, encryptStream, writeStream);
      await fs.rename(tempPath, filePath);
    } catch (error) {
      await this.cleanupTempFile(tempPath);
      throw error;
    }
  }

  private encryptFileBuffer(buffer: Buffer): Buffer {
    const key = this.createEncryptionKey();
    const iv = crypto.randomBytes(16);
    const cipher = crypto.createCipheriv("aes-256-cbc", key, iv);

    const encrypted = Buffer.concat([iv, cipher.update(buffer), cipher.final()]);

    return encrypted;
  }

  async downloadFile(objectName: string): Promise<Buffer> {
    const filePath = this.getFilePath(objectName);
    const encryptedBuffer = await fs.readFile(filePath);

    if (encryptedBuffer.length > 16) {
      try {
        return this.decryptFileBuffer(encryptedBuffer);
      } catch (error: unknown) {
        if (error instanceof Error) {
          console.warn("Failed to decrypt with new method, trying legacy format", error.message);
        }
        return this.decryptFileLegacy(encryptedBuffer);
      }
    }

    return this.decryptFileLegacy(encryptedBuffer);
  }

  private decryptFileBuffer(encryptedBuffer: Buffer): Buffer {
    const key = this.createEncryptionKey();
    const iv = encryptedBuffer.slice(0, 16);
    const encrypted = encryptedBuffer.slice(16);

    const decipher = crypto.createDecipheriv("aes-256-cbc", key, iv);

    return Buffer.concat([decipher.update(encrypted), decipher.final()]);
  }

  private decryptFileLegacy(encryptedBuffer: Buffer): Buffer {
    const CryptoJS = require("crypto-js");
    const decrypted = CryptoJS.AES.decrypt(encryptedBuffer.toString("utf8"), this.encryptionKey);
    return Buffer.from(decrypted.toString(CryptoJS.enc.Utf8), "base64");
  }

  async fileExists(objectName: string): Promise<boolean> {
    const filePath = this.getFilePath(objectName);
    try {
      await fs.access(filePath);
      return true;
    } catch {
      return false;
    }
  }

  validateUploadToken(token: string): { objectName: string } | null {
    const data = this.uploadTokens.get(token);
    if (!data || Date.now() > data.expiresAt) {
      this.uploadTokens.delete(token);
      return null;
    }
    return { objectName: data.objectName };
  }

  validateDownloadToken(token: string): { objectName: string; fileName?: string } | null {
    const data = this.downloadTokens.get(token);

    if (!data) {
      return null;
    }

    const now = Date.now();

    if (now > data.expiresAt) {
      this.downloadTokens.delete(token);
      return null;
    }

    return { objectName: data.objectName, fileName: data.fileName };
  }

  consumeUploadToken(token: string): void {
    this.uploadTokens.delete(token);
  }

  consumeDownloadToken(token: string): void {
    this.downloadTokens.delete(token);
  }

  /**
   * Clean up temporary file and its parent directory if empty
   */
  private async cleanupTempFile(tempPath: string): Promise<void> {
    try {
      // Remove the temp file
      await fs.unlink(tempPath);

      // Try to remove the parent directory if it's empty
      const tempDir = path.dirname(tempPath);
      try {
        const files = await fs.readdir(tempDir);
        if (files.length === 0) {
          await fs.rmdir(tempDir);
        }
      } catch (dirError: any) {
        // Ignore errors when trying to remove directory (might not be empty or might not exist)
        if (dirError.code !== "ENOTEMPTY" && dirError.code !== "ENOENT") {
          console.warn("Warning: Could not remove temp directory:", dirError.message);
        }
      }
    } catch (cleanupError: any) {
      if (cleanupError.code !== "ENOENT") {
        console.error("Error deleting temp file:", cleanupError);
      }
    }
  }

  /**
   * Clean up empty temporary directories periodically
   */
  private async cleanupEmptyTempDirs(): Promise<void> {
    try {
      const tempUploadsDir = directoriesConfig.tempUploads;

      // Check if temp-uploads directory exists
      try {
        await fs.access(tempUploadsDir);
      } catch {
        return; // Directory doesn't exist, nothing to clean
      }

      const items = await fs.readdir(tempUploadsDir);

      for (const item of items) {
        const itemPath = path.join(tempUploadsDir, item);

        try {
          const stat = await fs.stat(itemPath);

          if (stat.isDirectory()) {
            // Check if directory is empty
            const dirContents = await fs.readdir(itemPath);
            if (dirContents.length === 0) {
              await fs.rmdir(itemPath);
              console.log(`🧹 Cleaned up empty temp directory: ${itemPath}`);
            }
          } else if (stat.isFile()) {
            // Check if file is older than 1 hour (stale temp files)
            const oneHourAgo = Date.now() - 60 * 60 * 1000;
            if (stat.mtime.getTime() < oneHourAgo) {
              await fs.unlink(itemPath);
              console.log(`🧹 Cleaned up stale temp file: ${itemPath}`);
            }
          }
        } catch (error: any) {
          // Ignore errors for individual items
          if (error.code !== "ENOENT") {
            console.warn(`Warning: Could not process temp item ${itemPath}:`, error.message);
          }
        }
      }
    } catch (error) {
      console.error("Error during temp directory cleanup:", error);
    }
  }
}
