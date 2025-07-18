import * as fs from "fs";
import { Transform } from "stream";

/**
 * Information about an active download
 */
export interface DownloadInfo {
  id: string;
  objectName: string;
  startTime: number;
  fileSize: number;
  readStream?: fs.ReadStream;
  decryptStream?: Transform;
  isComplete: boolean;
  memoryUsageAtStart?: NodeJS.MemoryUsage;
  bytesTransferred?: number;
}

/**
 * Structured log entry for download events
 */
export interface DownloadLogEntry {
  timestamp: string;
  event: "download_started" | "download_completed" | "download_aborted" | "memory_usage" | "cleanup_completed";
  downloadId: string;
  objectName: string;
  fileSize?: number;
  duration?: number;
  throughput?: number;
  memoryUsage?: {
    rss: number;
    heapUsed: number;
    heapTotal: number;
    external: number;
    arrayBuffers: number;
  };
  memoryDelta?: {
    rss: number;
    heapUsed: number;
    heapTotal: number;
    external: number;
    arrayBuffers: number;
  };
  bytesTransferred?: number;
  reason?: string;
  error?: string;
}

/**
 * Singleton class to track and manage active downloads
 * Handles resource cleanup when downloads are aborted
 */
export class DownloadManager {
  private static instance: DownloadManager;
  private activeDownloads = new Map<string, DownloadInfo>();

  private constructor() {
    // Private constructor for singleton pattern
  }

  /**
   * Get the singleton instance of DownloadManager
   */
  public static getInstance(): DownloadManager {
    if (!DownloadManager.instance) {
      DownloadManager.instance = new DownloadManager();
    }
    return DownloadManager.instance;
  }

  /**
   * Start tracking a new download
   * @param id Unique download identifier
   * @param objectName File object name being downloaded
   * @param fileSize Size of the file in bytes
   * @returns The download ID
   */
  startDownload(id: string, objectName: string, fileSize: number): string {
    // Capture memory usage at start
    const memoryUsageAtStart = process.memoryUsage();

    const downloadInfo: DownloadInfo = {
      id,
      objectName,
      startTime: Date.now(),
      fileSize,
      isComplete: false,
      memoryUsageAtStart,
      bytesTransferred: 0,
    };

    this.activeDownloads.set(id, downloadInfo);

    // Log structured download start event
    this.logDownloadEvent({
      timestamp: new Date().toISOString(),
      event: "download_started",
      downloadId: id,
      objectName,
      fileSize,
      memoryUsage: {
        rss: memoryUsageAtStart.rss,
        heapUsed: memoryUsageAtStart.heapUsed,
        heapTotal: memoryUsageAtStart.heapTotal,
        external: memoryUsageAtStart.external,
        arrayBuffers: memoryUsageAtStart.arrayBuffers,
      },
    });

    // Legacy console log for backward compatibility
    console.log(`📥 Download started: ${id} (${objectName}, ${this.formatFileSize(fileSize)})`);

    return id;
  }

  /**
   * Register streams for a download to enable cleanup
   * @param id Download identifier
   * @param readStream File read stream
   * @param decryptStream Decryption transform stream
   */
  registerStreams(id: string, readStream: fs.ReadStream, decryptStream: Transform): void {
    const downloadInfo = this.activeDownloads.get(id);
    if (!downloadInfo) {
      console.warn(`⚠️ Attempted to register streams for unknown download: ${id}`);
      return;
    }

    downloadInfo.readStream = readStream;
    downloadInfo.decryptStream = decryptStream;

    console.log(`🔗 Streams registered for download: ${id}`);
  }

  /**
   * Mark a download as complete and clean up resources
   * @param id Download identifier
   */
  completeDownload(id: string): void {
    const downloadInfo = this.activeDownloads.get(id);
    if (!downloadInfo) {
      console.warn(`⚠️ Attempted to complete unknown download: ${id}`);
      return;
    }

    const duration = Date.now() - downloadInfo.startTime;
    const throughput = downloadInfo.fileSize / (duration / 1000); // bytes per second
    const currentMemoryUsage = process.memoryUsage();

    // Calculate memory delta if we have start memory usage
    let memoryDelta;
    if (downloadInfo.memoryUsageAtStart) {
      memoryDelta = {
        rss: currentMemoryUsage.rss - downloadInfo.memoryUsageAtStart.rss,
        heapUsed: currentMemoryUsage.heapUsed - downloadInfo.memoryUsageAtStart.heapUsed,
        heapTotal: currentMemoryUsage.heapTotal - downloadInfo.memoryUsageAtStart.heapTotal,
        external: currentMemoryUsage.external - downloadInfo.memoryUsageAtStart.external,
        arrayBuffers: currentMemoryUsage.arrayBuffers - downloadInfo.memoryUsageAtStart.arrayBuffers,
      };
    }

    downloadInfo.isComplete = true;

    // Log structured download completion event
    this.logDownloadEvent({
      timestamp: new Date().toISOString(),
      event: "download_completed",
      downloadId: id,
      objectName: downloadInfo.objectName,
      fileSize: downloadInfo.fileSize,
      duration,
      throughput,
      bytesTransferred: downloadInfo.bytesTransferred || downloadInfo.fileSize,
      memoryUsage: {
        rss: currentMemoryUsage.rss,
        heapUsed: currentMemoryUsage.heapUsed,
        heapTotal: currentMemoryUsage.heapTotal,
        external: currentMemoryUsage.external,
        arrayBuffers: currentMemoryUsage.arrayBuffers,
      },
      memoryDelta,
    });

    // Clean up streams
    this.cleanupStreams(downloadInfo);

    // Log cleanup completion
    this.logDownloadEvent({
      timestamp: new Date().toISOString(),
      event: "cleanup_completed",
      downloadId: id,
      objectName: downloadInfo.objectName,
      reason: "download_completed",
    });

    // Remove from active downloads
    this.activeDownloads.delete(id);

    // Legacy console log for backward compatibility
    console.log(
      `✅ Download completed: ${id} (${this.formatDuration(duration)}, ${this.formatThroughput(throughput)})`
    );
  }

  /**
   * Abort a download and clean up all associated resources
   * @param id Download identifier
   * @param reason Optional reason for abortion
   * @param error Optional error that caused the abortion
   */
  abortDownload(id: string, reason?: string, error?: Error): void {
    const downloadInfo = this.activeDownloads.get(id);
    if (!downloadInfo) {
      console.warn(`⚠️ Attempted to abort unknown download: ${id}`);
      return;
    }

    const duration = Date.now() - downloadInfo.startTime;
    const currentMemoryUsage = process.memoryUsage();

    // Calculate memory delta if we have start memory usage
    let memoryDelta;
    if (downloadInfo.memoryUsageAtStart) {
      memoryDelta = {
        rss: currentMemoryUsage.rss - downloadInfo.memoryUsageAtStart.rss,
        heapUsed: currentMemoryUsage.heapUsed - downloadInfo.memoryUsageAtStart.heapUsed,
        heapTotal: currentMemoryUsage.heapTotal - downloadInfo.memoryUsageAtStart.heapTotal,
        external: currentMemoryUsage.external - downloadInfo.memoryUsageAtStart.external,
        arrayBuffers: currentMemoryUsage.arrayBuffers - downloadInfo.memoryUsageAtStart.arrayBuffers,
      };
    }

    // Log structured download abortion event
    this.logDownloadEvent({
      timestamp: new Date().toISOString(),
      event: "download_aborted",
      downloadId: id,
      objectName: downloadInfo.objectName,
      fileSize: downloadInfo.fileSize,
      duration,
      bytesTransferred: downloadInfo.bytesTransferred || 0,
      memoryUsage: {
        rss: currentMemoryUsage.rss,
        heapUsed: currentMemoryUsage.heapUsed,
        heapTotal: currentMemoryUsage.heapTotal,
        external: currentMemoryUsage.external,
        arrayBuffers: currentMemoryUsage.arrayBuffers,
      },
      memoryDelta,
      reason: reason || "unknown",
      error: error?.message,
    });

    // Clean up streams
    this.cleanupStreams(downloadInfo);

    // Log cleanup completion
    this.logDownloadEvent({
      timestamp: new Date().toISOString(),
      event: "cleanup_completed",
      downloadId: id,
      objectName: downloadInfo.objectName,
      reason: "download_aborted",
    });

    // Remove from active downloads
    this.activeDownloads.delete(id);

    // Legacy console log for backward compatibility
    console.log(`❌ Download aborted: ${id} (${downloadInfo.objectName}, ${this.formatDuration(duration)})`);
  }

  /**
   * Get list of currently active downloads
   * @returns Array of active download information
   */
  getActiveDownloads(): DownloadInfo[] {
    return Array.from(this.activeDownloads.values());
  }

  /**
   * Get the count of active downloads
   * @returns Number of active downloads
   */
  getActiveDownloadCount(): number {
    return this.activeDownloads.size;
  }

  /**
   * Get information about a specific download
   * @param id Download identifier
   * @returns Download information or null if not found
   */
  getDownloadInfo(id: string): DownloadInfo | null {
    return this.activeDownloads.get(id) || null;
  }

  /**
   * Clean up orphaned downloads (for periodic cleanup)
   * This method can be called periodically to clean up any downloads
   * that may have been left in an inconsistent state
   * @returns Number of orphaned downloads cleaned up
   */
  cleanupOrphanedDownloads(): number {
    const now = Date.now();
    const maxAge = 30 * 60 * 1000; // 30 minutes
    let cleanedCount = 0;

    console.log(`🧹 Starting orphaned download cleanup check (${this.activeDownloads.size} active downloads)`);

    for (const [id, downloadInfo] of this.activeDownloads.entries()) {
      const age = now - downloadInfo.startTime;

      if (age > maxAge && !downloadInfo.isComplete) {
        console.log(`🧹 Cleaning up orphaned download: ${id} (age: ${this.formatDuration(age)})`);
        this.abortDownload(
          id,
          "orphaned_cleanup",
          new Error(`Download exceeded maximum age of ${this.formatDuration(maxAge)}`)
        );
        cleanedCount++;
      }
    }

    if (cleanedCount > 0) {
      console.log(`🧹 Cleaned up ${cleanedCount} orphaned downloads`);
    } else {
      console.log(`🧹 No orphaned downloads found`);
    }

    // Log memory usage after cleanup
    this.logMemoryUsageForActiveDownloads();

    return cleanedCount;
  }

  /**
   * Safely clean up streams associated with a download
   * @param downloadInfo Download information containing streams to clean up
   */
  private cleanupStreams(downloadInfo: DownloadInfo): void {
    try {
      // Clean up read stream
      if (downloadInfo.readStream) {
        this.safelyCloseStream(downloadInfo.readStream);
        downloadInfo.readStream = undefined;
      }

      // Clean up decrypt stream
      if (downloadInfo.decryptStream) {
        this.safelyCloseStream(downloadInfo.decryptStream);
        downloadInfo.decryptStream = undefined;
      }
    } catch (error) {
      console.error(`Error cleaning up streams for download ${downloadInfo.id}:`, error);
    }
  }

  /**
   * Safely close a stream with proper error handling
   * @param stream Stream to close
   */
  private safelyCloseStream(stream: NodeJS.ReadableStream | NodeJS.WritableStream | Transform): void {
    if (!stream) return;

    try {
      // Check if stream has destroy method (most Node.js streams do)
      if ("destroy" in stream && typeof stream.destroy === "function") {
        stream.destroy();
      }
      // Fallback to end method for writable streams
      else if ("end" in stream && typeof stream.end === "function") {
        (stream as NodeJS.WritableStream).end();
      }
      // Fallback to close method if available
      else if ("close" in stream && typeof stream.close === "function") {
        (stream as any).close();
      }
    } catch (error) {
      console.error("Error closing stream:", error);
    }
  }

  /**
   * Format file size for logging
   * @param bytes File size in bytes
   * @returns Formatted file size string
   */
  private formatFileSize(bytes: number): string {
    const units = ["B", "KB", "MB", "GB", "TB"];
    let size = bytes;
    let unitIndex = 0;

    while (size >= 1024 && unitIndex < units.length - 1) {
      size /= 1024;
      unitIndex++;
    }

    return `${size.toFixed(1)} ${units[unitIndex]}`;
  }

  /**
   * Format duration for logging
   * @param ms Duration in milliseconds
   * @returns Formatted duration string
   */
  private formatDuration(ms: number): string {
    if (ms < 1000) {
      return `${ms}ms`;
    } else if (ms < 60000) {
      return `${(ms / 1000).toFixed(1)}s`;
    } else {
      const minutes = Math.floor(ms / 60000);
      const seconds = ((ms % 60000) / 1000).toFixed(1);
      return `${minutes}m ${seconds}s`;
    }
  }

  /**
   * Format throughput for logging
   * @param bytesPerSecond Throughput in bytes per second
   * @returns Formatted throughput string
   */
  private formatThroughput(bytesPerSecond: number): string {
    return `${this.formatFileSize(bytesPerSecond)}/s`;
  }

  /**
   * Log a structured download event
   * @param logEntry Structured log entry for the download event
   */
  private logDownloadEvent(logEntry: DownloadLogEntry): void {
    // Log as structured JSON for monitoring tools
    console.log(`[DOWNLOAD_EVENT] ${JSON.stringify(logEntry)}`);

    // Also log memory usage separately if it's significant
    if (logEntry.memoryUsage && logEntry.event !== "cleanup_completed") {
      const memoryMB = {
        rss: Math.round(logEntry.memoryUsage.rss / 1024 / 1024),
        heapUsed: Math.round(logEntry.memoryUsage.heapUsed / 1024 / 1024),
        heapTotal: Math.round(logEntry.memoryUsage.heapTotal / 1024 / 1024),
        external: Math.round(logEntry.memoryUsage.external / 1024 / 1024),
        arrayBuffers: Math.round(logEntry.memoryUsage.arrayBuffers / 1024 / 1024),
      };

      console.log(
        `[MEMORY_USAGE] Download ${logEntry.downloadId}: RSS=${memoryMB.rss}MB, Heap=${memoryMB.heapUsed}/${memoryMB.heapTotal}MB, External=${memoryMB.external}MB, ArrayBuffers=${memoryMB.arrayBuffers}MB`
      );

      // Log memory delta if available
      if (logEntry.memoryDelta) {
        const deltaMB = {
          rss: Math.round(logEntry.memoryDelta.rss / 1024 / 1024),
          heapUsed: Math.round(logEntry.memoryDelta.heapUsed / 1024 / 1024),
          heapTotal: Math.round(logEntry.memoryDelta.heapTotal / 1024 / 1024),
          external: Math.round(logEntry.memoryDelta.external / 1024 / 1024),
          arrayBuffers: Math.round(logEntry.memoryDelta.arrayBuffers / 1024 / 1024),
        };

        const deltaSign = (val: number) => (val >= 0 ? "+" : "");
        console.log(
          `[MEMORY_DELTA] Download ${logEntry.downloadId}: RSS=${deltaSign(deltaMB.rss)}${deltaMB.rss}MB, Heap=${deltaSign(deltaMB.heapUsed)}${deltaMB.heapUsed}MB, External=${deltaSign(deltaMB.external)}${deltaMB.external}MB`
        );
      }
    }
  }

  /**
   * Update bytes transferred for a download (for progress tracking)
   * @param id Download identifier
   * @param bytesTransferred Number of bytes transferred so far
   */
  updateBytesTransferred(id: string, bytesTransferred: number): void {
    const downloadInfo = this.activeDownloads.get(id);
    if (downloadInfo) {
      downloadInfo.bytesTransferred = bytesTransferred;
    }
  }

  /**
   * Log current memory usage for all active downloads
   * This can be called periodically to monitor memory usage
   */
  logMemoryUsageForActiveDownloads(): void {
    if (this.activeDownloads.size === 0) {
      return;
    }

    const currentMemoryUsage = process.memoryUsage();
    const activeDownloadIds = Array.from(this.activeDownloads.keys());

    this.logDownloadEvent({
      timestamp: new Date().toISOString(),
      event: "memory_usage",
      downloadId: `active_downloads_${activeDownloadIds.length}`,
      objectName: `${activeDownloadIds.join(",")}`,
      memoryUsage: {
        rss: currentMemoryUsage.rss,
        heapUsed: currentMemoryUsage.heapUsed,
        heapTotal: currentMemoryUsage.heapTotal,
        external: currentMemoryUsage.external,
        arrayBuffers: currentMemoryUsage.arrayBuffers,
      },
    });

    console.log(`📊 Memory usage with ${this.activeDownloads.size} active downloads: ${activeDownloadIds.join(", ")}`);
  }

  /**
   * Get memory usage statistics for monitoring
   * @returns Object containing current memory usage and active download count
   */
  getMemoryUsageStats(): {
    memoryUsage: NodeJS.MemoryUsage;
    activeDownloadCount: number;
    activeDownloadIds: string[];
    totalFileSize: number;
  } {
    const memoryUsage = process.memoryUsage();
    const activeDownloads = Array.from(this.activeDownloads.values());
    const totalFileSize = activeDownloads.reduce((sum, download) => sum + download.fileSize, 0);

    return {
      memoryUsage,
      activeDownloadCount: activeDownloads.length,
      activeDownloadIds: activeDownloads.map((d) => d.id),
      totalFileSize,
    };
  }
}
