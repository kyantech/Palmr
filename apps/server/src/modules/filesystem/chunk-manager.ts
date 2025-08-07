import * as fs from "fs";
import * as path from "path";

import { getTempFilePath } from "../../config/directories.config";
import { FilesystemStorageProvider } from "../../providers/filesystem-storage.provider";

export interface ChunkMetadata {
  fileId: string;
  chunkIndex: number;
  totalChunks: number;
  chunkSize: number;
  totalSize: number;
  fileName: string;
  isLastChunk: boolean;
}

export interface ChunkInfo {
  fileId: string;
  fileName: string;
  totalSize: number;
  totalChunks: number;
  uploadedChunks: Set<number>;
  tempPath: string;
  createdAt: number;
}

export class ChunkManager {
  private static instance: ChunkManager;
  private activeUploads = new Map<string, ChunkInfo>();
  private finalizingUploads = new Set<string>(); // Track uploads currently being finalized
  private cleanupInterval: NodeJS.Timeout;

  private constructor() {
    // Cleanup expired uploads every 30 minutes
    this.cleanupInterval = setInterval(
      () => {
        this.cleanupExpiredUploads();
      },
      30 * 60 * 1000
    );
  }

  public static getInstance(): ChunkManager {
    if (!ChunkManager.instance) {
      ChunkManager.instance = new ChunkManager();
    }
    return ChunkManager.instance;
  }

  /**
   * Process a chunk upload with streaming
   */
  async processChunk(
    metadata: ChunkMetadata,
    inputStream: NodeJS.ReadableStream,
    originalObjectName: string
  ): Promise<{ isComplete: boolean; finalPath?: string }> {
    const startTime = Date.now();
    const { fileId, chunkIndex, totalChunks, fileName, totalSize, isLastChunk } = metadata;

    console.log(`Processing chunk ${chunkIndex + 1}/${totalChunks} for file ${fileName} (${fileId})`);

    let chunkInfo = this.activeUploads.get(fileId);
    if (!chunkInfo) {
      if (chunkIndex !== 0) {
        throw new Error("First chunk must be chunk 0");
      }

      const tempPath = getTempFilePath(fileId);
      chunkInfo = {
        fileId,
        fileName,
        totalSize,
        totalChunks,
        uploadedChunks: new Set(),
        tempPath,
        createdAt: Date.now(),
      };
      this.activeUploads.set(fileId, chunkInfo);
      console.log(`Created new upload session for ${fileName} at ${tempPath}`);
    }

    console.log(
      `Validating chunk ${chunkIndex} (total: ${totalChunks}, uploaded: ${Array.from(chunkInfo.uploadedChunks).join(",")})`
    );

    if (chunkIndex < 0 || chunkIndex >= totalChunks) {
      throw new Error(`Invalid chunk index: ${chunkIndex} (must be 0-${totalChunks - 1})`);
    }

    if (chunkInfo.uploadedChunks.has(chunkIndex)) {
      console.log(`Chunk ${chunkIndex} already uploaded, treating as success`);

      if (isLastChunk && chunkInfo.uploadedChunks.size === totalChunks) {
        if (this.finalizingUploads.has(fileId)) {
          console.log(`Upload ${fileId} is already being finalized, waiting...`);
          return { isComplete: false };
        }

        console.log(`All chunks uploaded, finalizing ${fileName}`);
        return await this.finalizeUpload(chunkInfo, metadata, originalObjectName);
      }

      return { isComplete: false };
    }

    const tempDir = path.dirname(chunkInfo.tempPath);
    await fs.promises.mkdir(tempDir, { recursive: true });
    console.log(`Temp directory ensured: ${tempDir}`);

    await this.writeChunkToFile(chunkInfo.tempPath, inputStream, chunkIndex === 0);

    chunkInfo.uploadedChunks.add(chunkIndex);

    try {
      const stats = await fs.promises.stat(chunkInfo.tempPath);
      const processingTime = Date.now() - startTime;
      console.log(
        `Chunk ${chunkIndex + 1}/${totalChunks} uploaded successfully in ${processingTime}ms. Temp file size: ${stats.size} bytes`
      );
    } catch (error) {
      console.warn(`Could not get temp file stats:`, error);
    }

    console.log(
      `Checking completion: isLastChunk=${isLastChunk}, uploadedChunks.size=${chunkInfo.uploadedChunks.size}, totalChunks=${totalChunks}`
    );

    if (isLastChunk && chunkInfo.uploadedChunks.size === totalChunks) {
      if (this.finalizingUploads.has(fileId)) {
        console.log(`Upload ${fileId} is already being finalized, waiting...`);
        return { isComplete: false };
      }

      console.log(`All chunks uploaded, finalizing ${fileName}`);

      const uploadedChunksArray = Array.from(chunkInfo.uploadedChunks).sort((a, b) => a - b);
      console.log(`Uploaded chunks in order: ${uploadedChunksArray.join(", ")}`);

      const expectedChunks = Array.from({ length: totalChunks }, (_, i) => i);
      const missingChunks = expectedChunks.filter((chunk) => !chunkInfo.uploadedChunks.has(chunk));

      if (missingChunks.length > 0) {
        throw new Error(`Missing chunks: ${missingChunks.join(", ")}`);
      }

      return await this.finalizeUpload(chunkInfo, metadata, originalObjectName);
    } else {
      console.log(
        `Not ready for finalization: isLastChunk=${isLastChunk}, uploadedChunks.size=${chunkInfo.uploadedChunks.size}, totalChunks=${totalChunks}`
      );
    }

    return { isComplete: false };
  }

  /**
   * Write chunk to file using streaming
   */
  private async writeChunkToFile(
    filePath: string,
    inputStream: NodeJS.ReadableStream,
    isFirstChunk: boolean
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      console.log(`Writing chunk to ${filePath} (first: ${isFirstChunk})`);

      if (isFirstChunk) {
        const writeStream = fs.createWriteStream(filePath, {
          highWaterMark: 64 * 1024 * 1024, // 64MB buffer for better performance
        });
        writeStream.on("error", (error) => {
          console.error("Write stream error:", error);
          reject(error);
        });
        writeStream.on("finish", () => {
          console.log("Write stream finished successfully");
          resolve();
        });
        inputStream.pipe(writeStream);
      } else {
        const writeStream = fs.createWriteStream(filePath, {
          flags: "a",
          highWaterMark: 64 * 1024 * 1024, // 64MB buffer for better performance
        });
        writeStream.on("error", (error) => {
          console.error("Write stream error:", error);
          reject(error);
        });
        writeStream.on("finish", () => {
          console.log("Write stream finished successfully");
          resolve();
        });
        inputStream.pipe(writeStream);
      }
    });
  }

  /**
   * Finalize upload by moving temp file to final location and encrypting (if enabled)
   */
  private async finalizeUpload(
    chunkInfo: ChunkInfo,
    metadata: ChunkMetadata,
    originalObjectName: string
  ): Promise<{ isComplete: boolean; finalPath: string }> {
    // Mark as finalizing to prevent race conditions
    this.finalizingUploads.add(chunkInfo.fileId);

    try {
      console.log(`Finalizing upload for ${chunkInfo.fileName}`);

      const tempStats = await fs.promises.stat(chunkInfo.tempPath);
      console.log(`Temp file size: ${tempStats.size} bytes, expected: ${chunkInfo.totalSize} bytes`);

      if (tempStats.size !== chunkInfo.totalSize) {
        console.warn(`Size mismatch! Temp: ${tempStats.size}, Expected: ${chunkInfo.totalSize}`);
      }

      const provider = FilesystemStorageProvider.getInstance();
      const finalObjectName = originalObjectName;
      const filePath = provider.getFilePath(finalObjectName);
      const dir = path.dirname(filePath);

      console.log(`Starting finalization: ${finalObjectName}`);

      await fs.promises.mkdir(dir, { recursive: true });

      const tempReadStream = fs.createReadStream(chunkInfo.tempPath, {
        highWaterMark: 64 * 1024 * 1024, // 64MB buffer for better performance
      });
      const writeStream = fs.createWriteStream(filePath, {
        highWaterMark: 64 * 1024 * 1024,
      });
      const encryptStream = provider.createEncryptStream();

      await new Promise<void>((resolve, reject) => {
        const startTime = Date.now();

        tempReadStream
          .pipe(encryptStream)
          .pipe(writeStream)
          .on("finish", () => {
            const duration = Date.now() - startTime;
            console.log(`File processed and saved to: ${filePath} in ${duration}ms`);
            resolve();
          })
          .on("error", (error) => {
            console.error("Error during processing:", error);
            reject(error);
          });
      });

      console.log(`File successfully uploaded and processed: ${finalObjectName}`);

      await this.cleanupTempFile(chunkInfo.tempPath);

      this.activeUploads.delete(chunkInfo.fileId);
      this.finalizingUploads.delete(chunkInfo.fileId);

      return { isComplete: true, finalPath: finalObjectName };
    } catch (error) {
      console.error("Error during finalization:", error);
      await this.cleanupTempFile(chunkInfo.tempPath);
      this.activeUploads.delete(chunkInfo.fileId);
      this.finalizingUploads.delete(chunkInfo.fileId);
      throw error;
    }
  }

  /**
   * Cleanup temporary file
   */
  private async cleanupTempFile(tempPath: string): Promise<void> {
    try {
      await fs.promises.access(tempPath);
      await fs.promises.unlink(tempPath);
      console.log(`Temp file cleaned up: ${tempPath}`);
    } catch (error: any) {
      if (error.code === "ENOENT") {
        console.log(`Temp file already cleaned up: ${tempPath}`);
      } else {
        console.warn(`Failed to cleanup temp file ${tempPath}:`, error);
      }
    }
  }

  /**
   * Cleanup expired uploads (older than 2 hours)
   */
  private async cleanupExpiredUploads(): Promise<void> {
    const now = Date.now();
    const maxAge = 2 * 60 * 60 * 1000; // 2 hours

    for (const [fileId, chunkInfo] of this.activeUploads.entries()) {
      if (now - chunkInfo.createdAt > maxAge) {
        console.log(`Cleaning up expired upload: ${fileId}`);
        await this.cleanupTempFile(chunkInfo.tempPath);
        this.activeUploads.delete(fileId);
        this.finalizingUploads.delete(fileId);
      }
    }
  }

  /**
   * Get upload progress
   */
  getUploadProgress(fileId: string): { uploaded: number; total: number; percentage: number } | null {
    const chunkInfo = this.activeUploads.get(fileId);
    if (!chunkInfo) return null;

    return {
      uploaded: chunkInfo.uploadedChunks.size,
      total: chunkInfo.totalChunks,
      percentage: Math.round((chunkInfo.uploadedChunks.size / chunkInfo.totalChunks) * 100),
    };
  }

  /**
   * Cancel upload
   */
  async cancelUpload(fileId: string): Promise<void> {
    const chunkInfo = this.activeUploads.get(fileId);
    if (chunkInfo) {
      await this.cleanupTempFile(chunkInfo.tempPath);
      this.activeUploads.delete(fileId);
      this.finalizingUploads.delete(fileId);
    }
  }

  /**
   * Cleanup on shutdown
   */
  destroy(): void {
    if (this.cleanupInterval) {
      clearInterval(this.cleanupInterval);
    }

    for (const [fileId, chunkInfo] of this.activeUploads.entries()) {
      this.cleanupTempFile(chunkInfo.tempPath);
    }
    this.activeUploads.clear();
    this.finalizingUploads.clear();
  }
}
