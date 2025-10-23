import axios from "axios";

export interface S3UploadOptions {
  file: File;
  presignedUrl: string;
  onProgress?: (progress: number) => void;
  signal?: AbortSignal;
}

export interface S3UploadResult {
  success: boolean;
  error?: string;
}

/**
 * Simple S3 upload utility using presigned URLs
 * No chunking needed - browser handles the upload directly to S3
 */
export class S3Uploader {
  /**
   * Upload a file directly to S3 using a presigned URL
   */
  static async uploadFile(options: S3UploadOptions): Promise<S3UploadResult> {
    const { file, presignedUrl, onProgress, signal } = options;

    try {
      // Calculate timeout based on file size
      // Base: 2 minutes + 1 minute per 100MB
      const fileSizeMB = file.size / (1024 * 1024);
      const baseTimeout = 120000; // 2 minutes
      const timeoutPerMB = 600; // 600ms per MB (~1 minute per 100MB)
      const calculatedTimeout = Math.max(baseTimeout, Math.ceil(fileSizeMB * timeoutPerMB));

      await axios.put(presignedUrl, file, {
        headers: {
          "Content-Type": file.type || "application/octet-stream",
        },
        signal,
        timeout: calculatedTimeout,
        maxContentLength: Infinity,
        maxBodyLength: Infinity,
        onUploadProgress: (progressEvent) => {
          if (onProgress && progressEvent.total) {
            const progress = (progressEvent.loaded / progressEvent.total) * 100;
            onProgress(Math.round(progress));
          }
        },
      });

      return {
        success: true,
      };
    } catch (error: any) {
      console.error("S3 upload failed:", error);

      let errorMessage = "Upload failed";
      if (axios.isAxiosError(error)) {
        if (error.code === "ECONNABORTED" || error.message?.includes("timeout")) {
          errorMessage = "Upload timeout - file is too large or connection is slow";
        } else if (error.response) {
          errorMessage = `Upload failed: ${error.response.status} ${error.response.statusText}`;
        } else if (error.request) {
          errorMessage = "No response from server - check your connection";
        }
      }

      return {
        success: false,
        error: errorMessage,
      };
    }
  }

  /**
   * Calculate upload timeout based on file size
   */
  static calculateTimeout(fileSizeBytes: number): number {
    const fileSizeMB = fileSizeBytes / (1024 * 1024);
    const baseTimeout = 120000; // 2 minutes
    const timeoutPerMB = 600; // 600ms per MB

    // For very large files (>500MB), add extra time
    if (fileSizeMB > 500) {
      const extraMB = fileSizeMB - 500;
      const extraMinutes = Math.ceil(extraMB / 100);
      return baseTimeout + fileSizeMB * timeoutPerMB + extraMinutes * 60000;
    }

    return Math.max(baseTimeout, Math.ceil(fileSizeMB * timeoutPerMB));
  }
}
