import * as crypto from "crypto";
import * as fsSync from "fs";
import * as fs from "fs/promises";
import * as path from "path";
import { Transform } from "stream";
import { pipeline } from "stream/promises";

import { directoriesConfig, getTempFilePath } from "../config/directories.config";
import { env } from "../env";
import { StorageProvider } from "../types/storage";

export class FilesystemStorageProvider implements StorageProvider {
  private static instance: FilesystemStorageProvider;
  private uploadsDir: string;
  private encryptionKey = env.ENCRYPTION_KEY;
  private isEncryptionDisabled = env.DISABLE_FILESYSTEM_ENCRYPTION === "true";
  private uploadTokens = new Map<string, { objectName: string; expiresAt: number }>();
  private downloadTokens = new Map<string, { objectName: string; expiresAt: number; fileName?: string }>();

  private constructor() {
    this.uploadsDir = directoriesConfig.uploads;

    if (!this.isEncryptionDisabled && !this.encryptionKey) {
      throw new Error(
        "Encryption is enabled but ENCRYPTION_KEY is not provided. " +
          "Please set ENCRYPTION_KEY environment variable or set DISABLE_FILESYSTEM_ENCRYPTION=true to disable encryption."
      );
    }

    this.ensureUploadsDir();
    setInterval(() => this.cleanExpiredTokens(), 5 * 60 * 1000);
    setInterval(() => this.cleanupEmptyTempDirs(), 10 * 60 * 1000);
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
    if (!this.encryptionKey) {
      throw new Error(
        "Encryption key is required when encryption is enabled. Please set ENCRYPTION_KEY environment variable."
      );
    }
    return crypto.scryptSync(this.encryptionKey, "salt", 32);
  }

  public createEncryptStream(): Transform {
    if (this.isEncryptionDisabled) {
      return new Transform({
        highWaterMark: 64 * 1024,
        transform(chunk, _encoding, callback) {
          this.push(chunk);
          callback();
        },
      });
    }

    const key = this.createEncryptionKey();
    const iv = crypto.randomBytes(16);
    const cipher = crypto.createCipheriv("aes-256-cbc", key, iv);

    let isFirstChunk = true;

    return new Transform({
      highWaterMark: 64 * 1024,
      transform(chunk, _encoding, callback) {
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
    if (this.isEncryptionDisabled) {
      return new Transform({
        highWaterMark: 64 * 1024,
        transform(chunk, _encoding, callback) {
          this.push(chunk);
          callback();
        },
      });
    }

    const key = this.createEncryptionKey();
    let iv: Buffer | null = null;
    let decipher: crypto.Decipher | null = null;
    let ivBuffer = Buffer.alloc(0);

    return new Transform({
      highWaterMark: 64 * 1024,
      transform(chunk, _encoding, callback) {
        try {
          if (!iv) {
            ivBuffer = Buffer.concat([ivBuffer, chunk]);

            if (ivBuffer.length >= 16) {
              iv = ivBuffer.subarray(0, 16);
              decipher = crypto.createDecipheriv("aes-256-cbc", key, iv);
              const remainingData = ivBuffer.subarray(16);
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

    const tempPath = getTempFilePath(objectName);
    const tempDir = path.dirname(tempPath);

    await fs.mkdir(tempDir, { recursive: true });

    const writeStream = fsSync.createWriteStream(tempPath);
    const encryptStream = this.createEncryptStream();

    try {
      await pipeline(inputStream, encryptStream, writeStream);
      await this.moveFile(tempPath, filePath);
    } catch (error) {
      await this.cleanupTempFile(tempPath);
      throw error;
    }
  }

  async downloadFile(objectName: string): Promise<Buffer> {
    const filePath = this.getFilePath(objectName);
    const fileBuffer = await fs.readFile(filePath);

    if (this.isEncryptionDisabled) {
      return fileBuffer;
    }

    if (fileBuffer.length > 16) {
      try {
        return this.decryptFileBuffer(fileBuffer);
      } catch (error: unknown) {
        if (error instanceof Error) {
          console.warn("Failed to decrypt with new method, trying legacy format", error.message);
        }
        return this.decryptFileLegacy(fileBuffer);
      }
    }

    return this.decryptFileLegacy(fileBuffer);
  }

  createDownloadStream(objectName: string): NodeJS.ReadableStream {
    const filePath = this.getFilePath(objectName);

    const streamOptions = {
      highWaterMark: 64 * 1024,
      autoDestroy: true,
      emitClose: true,
    };

    const fileStream = fsSync.createReadStream(filePath, streamOptions);

    if (this.isEncryptionDisabled) {
      this.setupStreamMemoryManagement(fileStream, objectName);
      return fileStream;
    }

    const decryptStream = this.createDecryptStream();
    const { PassThrough } = require("stream");
    const outputStream = new PassThrough(streamOptions);

    let isDestroyed = false;
    let memoryCheckInterval: NodeJS.Timeout;

    const cleanup = () => {
      if (isDestroyed) return;
      isDestroyed = true;

      if (memoryCheckInterval) {
        clearInterval(memoryCheckInterval);
      }

      try {
        if (fileStream && !fileStream.destroyed) {
          fileStream.destroy();
        }
        if (decryptStream && !decryptStream.destroyed) {
          decryptStream.destroy();
        }
        if (outputStream && !outputStream.destroyed) {
          outputStream.destroy();
        }
      } catch (error) {
        console.warn("Error during download stream cleanup:", error);
      }

      setImmediate(() => {
        if (global.gc) {
          global.gc();
        }
      });
    };

    memoryCheckInterval = setInterval(() => {
      const memUsage = process.memoryUsage();
      const memoryUsageMB = memUsage.heapUsed / 1024 / 1024;

      if (memoryUsageMB > 1024) {
        if (!fileStream.readableFlowing) return;

        console.warn(
          `[MEMORY THROTTLE] ${objectName} - Pausing stream due to high memory usage: ${memoryUsageMB.toFixed(2)}MB`
        );
        fileStream.pause();

        if (global.gc) {
          global.gc();
        }

        setTimeout(() => {
          if (!isDestroyed && fileStream && !fileStream.destroyed) {
            fileStream.resume();
            console.log(`[MEMORY THROTTLE] ${objectName} - Stream resumed`);
          }
        }, 100);
      }
    }, 1000);

    fileStream.on("error", (error: any) => {
      console.error("File stream error:", error);
      cleanup();
    });

    decryptStream.on("error", (error: any) => {
      console.error("Decrypt stream error:", error);
      cleanup();
    });

    outputStream.on("error", (error: any) => {
      console.error("Output stream error:", error);
      cleanup();
    });

    outputStream.on("close", cleanup);
    outputStream.on("finish", cleanup);

    outputStream.on("pipe", (src: any) => {
      if (src && src.on) {
        src.on("close", cleanup);
        src.on("error", cleanup);
      }
    });

    pipeline(fileStream, decryptStream, outputStream)
      .then(() => {})
      .catch((error: any) => {
        console.error("Pipeline error during download:", error);
        cleanup();
      });

    this.setupStreamMemoryManagement(outputStream, objectName);
    return outputStream;
  }

  private setupStreamMemoryManagement(stream: NodeJS.ReadableStream, objectName: string): void {
    let lastMemoryLog = 0;

    stream.on("data", () => {
      const now = Date.now();
      if (now - lastMemoryLog > 30000) {
        FilesystemStorageProvider.logMemoryUsage(`Active download: ${objectName}`);
        lastMemoryLog = now;
      }
    });

    stream.on("end", () => {
      FilesystemStorageProvider.logMemoryUsage(`Download completed: ${objectName}`);
      setImmediate(() => {
        if (global.gc) {
          global.gc();
        }
      });
    });

    stream.on("close", () => {
      FilesystemStorageProvider.logMemoryUsage(`Download closed: ${objectName}`);
    });
  }

  async createDownloadRangeStream(objectName: string, start: number, end: number): Promise<NodeJS.ReadableStream> {
    if (!this.isEncryptionDisabled) {
      return this.createRangeStreamFromDecrypted(objectName, start, end);
    }

    const filePath = this.getFilePath(objectName);
    return fsSync.createReadStream(filePath, { start, end });
  }

  private createRangeStreamFromDecrypted(objectName: string, start: number, end: number): NodeJS.ReadableStream {
    const { Transform, PassThrough } = require("stream");
    const filePath = this.getFilePath(objectName);
    const fileStream = fsSync.createReadStream(filePath);
    const decryptStream = this.createDecryptStream();
    const rangeStream = new PassThrough();

    let bytesRead = 0;
    let rangeEnded = false;
    let isDestroyed = false;

    const rangeTransform = new Transform({
      transform(chunk: Buffer, encoding: any, callback: any) {
        if (rangeEnded || isDestroyed) {
          callback();
          return;
        }

        const chunkStart = bytesRead;
        const chunkEnd = bytesRead + chunk.length - 1;
        bytesRead += chunk.length;

        if (chunkEnd < start) {
          callback();
          return;
        }

        if (chunkStart > end) {
          rangeEnded = true;
          this.end();
          callback();
          return;
        }

        let sliceStart = 0;
        let sliceEnd = chunk.length;

        if (chunkStart < start) {
          sliceStart = start - chunkStart;
        }

        if (chunkEnd > end) {
          sliceEnd = end - chunkStart + 1;
          rangeEnded = true;
        }

        const slicedChunk = chunk.slice(sliceStart, sliceEnd);
        this.push(slicedChunk);

        if (rangeEnded) {
          this.end();
        }

        callback();
      },

      flush(callback: any) {
        if (global.gc) {
          global.gc();
        }
        callback();
      },
    });

    const cleanup = () => {
      if (isDestroyed) return;
      isDestroyed = true;

      try {
        if (fileStream && !fileStream.destroyed) {
          fileStream.destroy();
        }
        if (decryptStream && !decryptStream.destroyed) {
          decryptStream.destroy();
        }
        if (rangeTransform && !rangeTransform.destroyed) {
          rangeTransform.destroy();
        }
        if (rangeStream && !rangeStream.destroyed) {
          rangeStream.destroy();
        }
      } catch (error) {
        console.warn("Error during stream cleanup:", error);
      }

      if (global.gc) {
        global.gc();
      }
    };

    fileStream.on("error", cleanup);
    decryptStream.on("error", cleanup);
    rangeTransform.on("error", cleanup);
    rangeStream.on("error", cleanup);

    rangeStream.on("close", cleanup);
    rangeStream.on("end", cleanup);

    rangeStream.on("pipe", (src: any) => {
      if (src && src.on) {
        src.on("close", cleanup);
        src.on("error", cleanup);
      }
    });

    fileStream.pipe(decryptStream).pipe(rangeTransform).pipe(rangeStream);

    return rangeStream;
  }

  private decryptFileBuffer(encryptedBuffer: Buffer): Buffer {
    const key = this.createEncryptionKey();
    const iv = encryptedBuffer.slice(0, 16);
    const encrypted = encryptedBuffer.slice(16);

    const decipher = crypto.createDecipheriv("aes-256-cbc", key, iv);

    return Buffer.concat([decipher.update(encrypted), decipher.final()]);
  }

  private decryptFileLegacy(encryptedBuffer: Buffer): Buffer {
    if (!this.encryptionKey) {
      throw new Error(
        "Encryption key is required when encryption is enabled. Please set ENCRYPTION_KEY environment variable."
      );
    }
    const CryptoJS = require("crypto-js");
    const decrypted = CryptoJS.AES.decrypt(encryptedBuffer.toString("utf8"), this.encryptionKey);
    return Buffer.from(decrypted.toString(CryptoJS.enc.Utf8), "base64");
  }

  static logMemoryUsage(context: string = "Unknown"): void {
    const memUsage = process.memoryUsage();
    const formatBytes = (bytes: number) => {
      const mb = bytes / 1024 / 1024;
      return `${mb.toFixed(2)} MB`;
    };

    const rssInMB = memUsage.rss / 1024 / 1024;
    const heapUsedInMB = memUsage.heapUsed / 1024 / 1024;

    if (rssInMB > 1024 || heapUsedInMB > 512) {
      console.warn(`[MEMORY WARNING] ${context} - High memory usage detected:`);
      console.warn(`  RSS: ${formatBytes(memUsage.rss)}`);
      console.warn(`  Heap Used: ${formatBytes(memUsage.heapUsed)}`);
      console.warn(`  Heap Total: ${formatBytes(memUsage.heapTotal)}`);
      console.warn(`  External: ${formatBytes(memUsage.external)}`);

      if (global.gc) {
        console.warn("  Forcing garbage collection...");
        global.gc();

        const afterGC = process.memoryUsage();
        console.warn(`  After GC - RSS: ${formatBytes(afterGC.rss)}, Heap: ${formatBytes(afterGC.heapUsed)}`);
      }
    } else {
      console.log(
        `[MEMORY INFO] ${context} - RSS: ${formatBytes(memUsage.rss)}, Heap: ${formatBytes(memUsage.heapUsed)}`
      );
    }
  }

  static forceGarbageCollection(context: string = "Manual"): void {
    if (global.gc) {
      const beforeGC = process.memoryUsage();
      global.gc();
      const afterGC = process.memoryUsage();

      const formatBytes = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(2)} MB`;

      console.log(`[GC] ${context} - Before: RSS ${formatBytes(beforeGC.rss)}, Heap ${formatBytes(beforeGC.heapUsed)}`);
      console.log(`[GC] ${context} - After:  RSS ${formatBytes(afterGC.rss)}, Heap ${formatBytes(afterGC.heapUsed)}`);

      const rssSaved = beforeGC.rss - afterGC.rss;
      const heapSaved = beforeGC.heapUsed - afterGC.heapUsed;

      if (rssSaved > 0 || heapSaved > 0) {
        console.log(`[GC] ${context} - Freed: RSS ${formatBytes(rssSaved)}, Heap ${formatBytes(heapSaved)}`);
      }
    } else {
      console.warn(`[GC] ${context} - Garbage collection not available. Start Node.js with --expose-gc flag.`);
    }
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

  // Tokens are automatically cleaned up by cleanExpiredTokens() every 5 minutes
  // No need to manually consume tokens - allows reuse for previews, range requests, etc.

  private async cleanupTempFile(tempPath: string): Promise<void> {
    try {
      await fs.unlink(tempPath);

      const tempDir = path.dirname(tempPath);
      try {
        const files = await fs.readdir(tempDir);
        if (files.length === 0) {
          await fs.rmdir(tempDir);
        }
      } catch (dirError: any) {
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

  private async cleanupEmptyTempDirs(): Promise<void> {
    try {
      const tempUploadsDir = directoriesConfig.tempUploads;

      try {
        await fs.access(tempUploadsDir);
      } catch {
        return;
      }

      const items = await fs.readdir(tempUploadsDir);

      for (const item of items) {
        const itemPath = path.join(tempUploadsDir, item);

        try {
          const stat = await fs.stat(itemPath);

          if (stat.isDirectory()) {
            const dirContents = await fs.readdir(itemPath);
            if (dirContents.length === 0) {
              await fs.rmdir(itemPath);
              console.log(`🧹 Cleaned up empty temp directory: ${itemPath}`);
            }
          } else if (stat.isFile()) {
            const oneHourAgo = Date.now() - 60 * 60 * 1000;
            if (stat.mtime.getTime() < oneHourAgo) {
              await fs.unlink(itemPath);
              console.log(`🧹 Cleaned up stale temp file: ${itemPath}`);
            }
          }
        } catch (error: any) {
          if (error.code !== "ENOENT") {
            console.warn(`Warning: Could not process temp item ${itemPath}:`, error.message);
          }
        }
      }
    } catch (error) {
      console.error("Error during temp directory cleanup:", error);
    }
  }

  private async moveFile(src: string, dest: string): Promise<void> {
    try {
      await fs.rename(src, dest);
    } catch (err: any) {
      if (err.code === "EXDEV") {
        // cross-device: fallback to copy + delete
        await fs.copyFile(src, dest);
        await fs.unlink(src);
      } else {
        throw err;
      }
    }
  }
}
