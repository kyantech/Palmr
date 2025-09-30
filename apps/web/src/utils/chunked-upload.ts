import axios from "axios";

export interface ChunkedUploadOptions {
  file: File;
  url: string;
  chunkSize?: number;
  onProgress?: (progress: number) => void;
  onChunkComplete?: (chunkIndex: number, totalChunks: number) => void;
  signal?: AbortSignal;
  isS3Enabled?: boolean;
}

export interface ChunkedUploadResult {
  success: boolean;
  objectName?: string;
  finalObjectName?: string;
  error?: string;
}

export class ChunkedUploader {
  private static defaultChunkSizeInBytes = 100 * 1024 * 1024; // 100MB

  /**
   * Upload a file in chunks with streaming
   */
  static async uploadFile(options: ChunkedUploadOptions): Promise<ChunkedUploadResult> {
    const { file, url, chunkSize, onProgress, onChunkComplete, signal } = options;

    if (!this.shouldUseChunkedUpload(file.size, options.isS3Enabled)) {
      throw new Error(
        `File ${file.name} (${(file.size / (1024 * 1024)).toFixed(2)}MB) should not use chunked upload. Use regular upload instead.`
      );
    }

    const optimalChunkSize = chunkSize || this.calculateOptimalChunkSize(file.size);

    try {
      const fileId = this.generateFileId();

      const totalChunks = Math.ceil(file.size / optimalChunkSize);

      const uploadedChunks = new Set<number>();
      let completedChunks = 0;
      let lastChunkResponse: any = null;

      for (let chunkIndex = 0; chunkIndex < totalChunks; chunkIndex++) {
        if (signal?.aborted) {
          throw new Error("Upload cancelled");
        }

        const start = chunkIndex * optimalChunkSize;
        const end = Math.min(start + optimalChunkSize, file.size);
        const chunk = file.slice(start, end);
        const isLastChunk = chunkIndex === totalChunks - 1;

        let retries = 0;
        const maxRetries = 3;
        let chunkUploaded = false;

        while (retries < maxRetries && !chunkUploaded) {
          try {
            const response = await this.uploadChunk({
              fileId,
              chunk,
              chunkIndex,
              totalChunks,
              chunkSize: optimalChunkSize,
              totalSize: file.size,
              fileName: file.name,
              isLastChunk,
              url,
              signal,
            });

            if (isLastChunk) {
              lastChunkResponse = response;
            }

            chunkUploaded = true;
          } catch (error: any) {
            retries++;

            if (
              error.response?.status === 400 &&
              (error.response?.data?.error?.includes("already uploaded") ||
                error.response?.data?.details?.includes("already uploaded"))
            ) {
              chunkUploaded = true;
              break;
            }

            console.warn(`Chunk ${chunkIndex + 1} failed (attempt ${retries}/${maxRetries}):`, error.message);

            if (retries >= maxRetries) {
              throw error;
            }

            const backoffDelay = error.message?.includes("timeout") ? 2000 * retries : 1000 * retries;
            await new Promise((resolve) => setTimeout(resolve, backoffDelay));
          }
        }

        if (!chunkUploaded) {
          throw new Error(`Failed to upload chunk ${chunkIndex + 1} after ${maxRetries} attempts`);
        }

        uploadedChunks.add(chunkIndex);
        completedChunks++;

        const progress = Math.round((completedChunks / totalChunks) * 100);
        onProgress?.(progress);
        onChunkComplete?.(chunkIndex, totalChunks);

        if (!isLastChunk) {
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }

      await new Promise((resolve) => setTimeout(resolve, 500));

      return {
        success: true,
        finalObjectName: lastChunkResponse?.finalObjectName || lastChunkResponse?.objectName,
      };
    } catch (error: any) {
      console.error("Chunked upload failed:", error);
      return {
        success: false,
        error: error.message || "Upload failed",
      };
    }
  }

  /**
   * Upload a single chunk
   */
  private static async uploadChunk({
    fileId,
    chunk,
    chunkIndex,
    totalChunks,
    chunkSize,
    totalSize,
    fileName,
    isLastChunk,
    url,
    signal,
  }: {
    fileId: string;
    chunk: Blob;
    chunkIndex: number;
    totalChunks: number;
    chunkSize: number;
    totalSize: number;
    fileName: string;
    isLastChunk: boolean;
    url: string;
    signal?: AbortSignal;
  }): Promise<any> {
    const headers = {
      "Content-Type": "application/octet-stream",
      "X-File-Id": fileId,
      "X-Chunk-Index": chunkIndex.toString(),
      "X-Total-Chunks": totalChunks.toString(),
      "X-Chunk-Size": chunkSize.toString(),
      "X-Total-Size": totalSize.toString(),
      "X-File-Name": fileName,
      "X-Is-Last-Chunk": isLastChunk.toString(),
    };

    try {
      const timeoutPer100MB = 120000; // 120 seconds per 100MB
      const chunkSizeMB = chunk.size / (1024 * 1024);
      const calculatedTimeout = Math.max(60000, Math.ceil(chunkSizeMB / 100) * timeoutPer100MB);

      const response = await axios.put(url, chunk, {
        headers,
        signal,
        timeout: calculatedTimeout,
        maxContentLength: Infinity,
        maxBodyLength: Infinity,
      });

      if (response.status !== 200) {
        throw new Error(`Failed to upload chunk ${chunkIndex}: ${response.statusText}`);
      }

      return response.data;
    } catch (error: any) {
      if (
        error.response?.status === 400 &&
        (error.response?.data?.error?.includes("already uploaded") ||
          error.response?.data?.details?.includes("already uploaded"))
      ) {
        return error.response.data;
      }

      if (error.code === "ECONNABORTED" || error.message?.includes("timeout")) {
        console.warn(`Chunk ${chunkIndex + 1} upload timed out, will retry`);
        throw new Error(`Upload timeout for chunk ${chunkIndex + 1}`);
      }

      throw error;
    }
  }

  /**
   * Get upload progress
   */
  static async getUploadProgress(fileId: string): Promise<{
    uploaded: number;
    total: number;
    percentage: number;
  } | null> {
    try {
      const response = await axios.get(`/api/filesystem/upload-progress/${fileId}`);
      return response.data;
    } catch (error) {
      console.warn("Failed to get upload progress:", error);
      return null;
    }
  }

  /**
   * Cancel upload
   */
  static async cancelUpload(fileId: string): Promise<void> {
    try {
      await axios.delete(`/api/filesystem/cancel-upload/${fileId}`);
    } catch (error) {
      console.warn("Failed to cancel upload:", error);
    }
  }

  /**
   * Generate unique file ID
   */
  private static generateFileId(): string {
    return `${Date.now()}-${Math.random().toString(36).substring(2, 15)}`;
  }

  /**
   * Check if file should use chunked upload
   * Only use chunked upload for filesystem storage, not for S3
   */
  static shouldUseChunkedUpload(fileSize: number, isS3Enabled?: boolean): boolean {
    if (isS3Enabled) {
      return false;
    }

    const threshold = this.getConfiguredChunkSize() || this.defaultChunkSizeInBytes;
    const shouldUse = fileSize > threshold;

    return shouldUse;
  }

  /**
   * Calculate optimal chunk size based on file size
   */
  static calculateOptimalChunkSize(fileSize: number): number {
    const configuredChunkSize = this.getConfiguredChunkSize();
    const chunkSize = configuredChunkSize || this.defaultChunkSizeInBytes;

    if (fileSize <= chunkSize) {
      throw new Error(
        `calculateOptimalChunkSize should not be called for files <= ${chunkSize}. File size: ${(fileSize / (1024 * 1024)).toFixed(2)}MB`
      );
    }

    if (configuredChunkSize) {
      return configuredChunkSize;
    }

    // For files > 1GB, use 150MB chunks
    if (fileSize > 1024 * 1024 * 1024) {
      return 150 * 1024 * 1024;
    }

    // For files > 500MB, use 100MB chunks
    if (fileSize > 500 * 1024 * 1024) {
      return 100 * 1024 * 1024;
    }

    // For files > 100MB, use 75MB chunks (minimum for chunked upload)
    return 75 * 1024 * 1024;
  }

  private static getConfiguredChunkSize(): number | null {
    const configuredChunkSizeMb = process.env.NEXT_PUBLIC_UPLOAD_CHUNK_SIZE_MB;

    if (!configuredChunkSizeMb) {
      return null;
    }

    const parsedValue = Number(configuredChunkSizeMb);

    if (Number.isNaN(parsedValue) || parsedValue <= 0) {
      console.warn(
        `Invalid NEXT_PUBLIC_UPLOAD_CHUNK_SIZE_MB value: ${configuredChunkSizeMb}. Falling back to optimal chunk size.`
      );

      return null;
    }

    return Math.floor(parsedValue * 1024 * 1024);
  }
}
