import { ActiveDownloadInfo, DownloadSlotRequest, QueuedDownloadInfo, QueueStatus } from "../types/download-queue";

interface QueuedDownload {
  downloadId: string;
  queuedAt: number;
  resolve: () => void;
  reject: (error: Error) => void;
  metadata?: DownloadSlotRequest;
}

export class DownloadMemoryManager {
  private static instance: DownloadMemoryManager;
  private activeDownloads = new Map<string, ActiveDownloadInfo>();
  private downloadQueue: QueuedDownload[] = [];
  private maxConcurrentDownloads: number;
  private memoryThresholdMB: number;
  private maxQueueSize: number;
  private cleanupInterval: NodeJS.Timeout;
  private isAutoScalingEnabled: boolean;
  private minFileSizeGB: number;

  private constructor() {
    const { env } = require("../env");

    const totalMemoryGB = require("os").totalmem() / 1024 ** 3;
    this.isAutoScalingEnabled = env.DOWNLOAD_AUTO_SCALE === "true";

    if (env.DOWNLOAD_MAX_CONCURRENT !== undefined) {
      this.maxConcurrentDownloads = env.DOWNLOAD_MAX_CONCURRENT;
    } else if (this.isAutoScalingEnabled) {
      this.maxConcurrentDownloads = this.calculateDefaultConcurrentDownloads(totalMemoryGB);
    } else {
      this.maxConcurrentDownloads = 3;
    }

    if (env.DOWNLOAD_MEMORY_THRESHOLD_MB !== undefined) {
      this.memoryThresholdMB = env.DOWNLOAD_MEMORY_THRESHOLD_MB;
    } else if (this.isAutoScalingEnabled) {
      this.memoryThresholdMB = this.calculateDefaultMemoryThreshold(totalMemoryGB);
    } else {
      this.memoryThresholdMB = 1024;
    }

    if (env.DOWNLOAD_QUEUE_SIZE !== undefined) {
      this.maxQueueSize = env.DOWNLOAD_QUEUE_SIZE;
    } else if (this.isAutoScalingEnabled) {
      this.maxQueueSize = this.calculateDefaultQueueSize(totalMemoryGB);
    } else {
      this.maxQueueSize = 15;
    }

    if (env.DOWNLOAD_MIN_FILE_SIZE_GB !== undefined) {
      this.minFileSizeGB = env.DOWNLOAD_MIN_FILE_SIZE_GB;
    } else {
      this.minFileSizeGB = 3.0;
    }

    this.validateConfiguration();

    console.log(`[DOWNLOAD MANAGER] Configuration loaded:`);
    console.log(`  System Memory: ${totalMemoryGB.toFixed(1)}GB`);
    console.log(
      `  Max Concurrent: ${this.maxConcurrentDownloads} ${env.DOWNLOAD_MAX_CONCURRENT !== undefined ? "(ENV)" : "(AUTO)"}`
    );
    console.log(
      `  Memory Threshold: ${this.memoryThresholdMB}MB ${env.DOWNLOAD_MEMORY_THRESHOLD_MB !== undefined ? "(ENV)" : "(AUTO)"}`
    );
    console.log(`  Queue Size: ${this.maxQueueSize} ${env.DOWNLOAD_QUEUE_SIZE !== undefined ? "(ENV)" : "(AUTO)"}`);
    console.log(
      `  Min File Size: ${this.minFileSizeGB}GB ${env.DOWNLOAD_MIN_FILE_SIZE_GB !== undefined ? "(ENV)" : "(DEFAULT)"}`
    );
    console.log(`  Auto-scaling: ${this.isAutoScalingEnabled ? "enabled" : "disabled"}`);

    this.cleanupInterval = setInterval(() => {
      this.cleanupStaleDownloads();
    }, 30000);
  }

  public static getInstance(): DownloadMemoryManager {
    if (!DownloadMemoryManager.instance) {
      DownloadMemoryManager.instance = new DownloadMemoryManager();
    }
    return DownloadMemoryManager.instance;
  }

  private calculateDefaultConcurrentDownloads(totalMemoryGB: number): number {
    if (totalMemoryGB > 16) return 10;
    if (totalMemoryGB > 8) return 5;
    if (totalMemoryGB > 4) return 3;
    if (totalMemoryGB > 2) return 2;
    return 1;
  }

  private calculateDefaultMemoryThreshold(totalMemoryGB: number): number {
    if (totalMemoryGB > 16) return 4096; // 4GB
    if (totalMemoryGB > 8) return 2048; // 2GB
    if (totalMemoryGB > 4) return 1024; // 1GB
    if (totalMemoryGB > 2) return 512; // 512MB
    return 256; // 256MB
  }

  private calculateDefaultQueueSize(totalMemoryGB: number): number {
    if (totalMemoryGB > 16) return 50; // Large queue for powerful servers
    if (totalMemoryGB > 8) return 25; // Medium queue
    if (totalMemoryGB > 4) return 15; // Small queue
    if (totalMemoryGB > 2) return 10; // Very small queue
    return 5; // Minimal queue
  }

  private validateConfiguration(): void {
    const warnings: string[] = [];
    const errors: string[] = [];

    if (this.maxConcurrentDownloads < 1) {
      errors.push(`DOWNLOAD_MAX_CONCURRENT must be >= 1, got: ${this.maxConcurrentDownloads}`);
    }
    if (this.maxConcurrentDownloads > 50) {
      warnings.push(
        `DOWNLOAD_MAX_CONCURRENT is very high (${this.maxConcurrentDownloads}), this may cause performance issues`
      );
    }

    if (this.memoryThresholdMB < 128) {
      warnings.push(
        `DOWNLOAD_MEMORY_THRESHOLD_MB is very low (${this.memoryThresholdMB}MB), downloads may be throttled frequently`
      );
    }
    if (this.memoryThresholdMB > 16384) {
      warnings.push(
        `DOWNLOAD_MEMORY_THRESHOLD_MB is very high (${this.memoryThresholdMB}MB), system may run out of memory`
      );
    }

    if (this.maxQueueSize < 1) {
      errors.push(`DOWNLOAD_QUEUE_SIZE must be >= 1, got: ${this.maxQueueSize}`);
    }
    if (this.maxQueueSize > 1000) {
      warnings.push(`DOWNLOAD_QUEUE_SIZE is very high (${this.maxQueueSize}), this may consume significant memory`);
    }

    if (this.minFileSizeGB < 0.1) {
      warnings.push(
        `DOWNLOAD_MIN_FILE_SIZE_GB is very low (${this.minFileSizeGB}GB), most downloads will use memory management`
      );
    }
    if (this.minFileSizeGB > 50) {
      warnings.push(
        `DOWNLOAD_MIN_FILE_SIZE_GB is very high (${this.minFileSizeGB}GB), memory management may rarely activate`
      );
    }

    const recommendedQueueSize = this.maxConcurrentDownloads * 5;
    if (this.maxQueueSize < this.maxConcurrentDownloads) {
      warnings.push(
        `DOWNLOAD_QUEUE_SIZE (${this.maxQueueSize}) is smaller than DOWNLOAD_MAX_CONCURRENT (${this.maxConcurrentDownloads})`
      );
    } else if (this.maxQueueSize < recommendedQueueSize) {
      warnings.push(
        `DOWNLOAD_QUEUE_SIZE (${this.maxQueueSize}) might be too small. Recommended: ${recommendedQueueSize} (5x concurrent downloads)`
      );
    }

    if (warnings.length > 0) {
      console.warn(`[DOWNLOAD MANAGER] Configuration warnings:`);
      warnings.forEach((warning) => console.warn(`  - ${warning}`));
    }

    if (errors.length > 0) {
      console.error(`[DOWNLOAD MANAGER] Configuration errors:`);
      errors.forEach((error) => console.error(`  - ${error}`));
      throw new Error(`Invalid download manager configuration: ${errors.join(", ")}`);
    }
  }

  public async requestDownloadSlot(downloadId: string, metadata?: DownloadSlotRequest): Promise<void> {
    if (metadata?.fileSize) {
      const fileSizeGB = metadata.fileSize / 1024 ** 3;
      if (fileSizeGB < this.minFileSizeGB) {
        console.log(
          `[DOWNLOAD MANAGER] File ${metadata.fileName || "unknown"} (${fileSizeGB.toFixed(2)}GB) below threshold (${this.minFileSizeGB}GB), bypassing queue`
        );
        return Promise.resolve();
      }
    }

    if (this.canStartImmediately()) {
      console.log(`[DOWNLOAD MANAGER] Immediate start: ${downloadId}`);
      return Promise.resolve();
    }

    if (this.downloadQueue.length >= this.maxQueueSize) {
      const error = new Error(`Download queue is full: ${this.downloadQueue.length}/${this.maxQueueSize}`);
      throw error;
    }

    return new Promise<void>((resolve, reject) => {
      const queuedDownload: QueuedDownload = {
        downloadId,
        queuedAt: Date.now(),
        resolve,
        reject,
        metadata,
      };

      this.downloadQueue.push(queuedDownload);

      const position = this.downloadQueue.length;
      console.log(`[DOWNLOAD MANAGER] Queued: ${downloadId} (Position: ${position}/${this.maxQueueSize})`);

      if (metadata?.fileName && metadata?.fileSize) {
        const sizeMB = (metadata.fileSize / (1024 * 1024)).toFixed(1);
        console.log(`[DOWNLOAD MANAGER] Queued file: ${metadata.fileName} (${sizeMB}MB)`);
      }
    });
  }

  private canStartImmediately(): boolean {
    const currentMemoryMB = this.getCurrentMemoryUsage();

    if (currentMemoryMB > this.memoryThresholdMB) {
      return false;
    }

    if (this.activeDownloads.size >= this.maxConcurrentDownloads) {
      return false;
    }

    return true;
  }

  public canStartDownload(): { allowed: boolean; reason?: string } {
    if (this.canStartImmediately()) {
      return { allowed: true };
    }

    const currentMemoryMB = this.getCurrentMemoryUsage();

    if (currentMemoryMB > this.memoryThresholdMB) {
      return {
        allowed: false,
        reason: `Memory usage too high: ${currentMemoryMB.toFixed(0)}MB > ${this.memoryThresholdMB}MB`,
      };
    }

    return {
      allowed: false,
      reason: `Too many concurrent downloads: ${this.activeDownloads.size}/${this.maxConcurrentDownloads}`,
    };
  }

  public startDownload(downloadId: string): void {
    const memUsage = process.memoryUsage();
    this.activeDownloads.set(downloadId, {
      startTime: Date.now(),
      memoryAtStart: memUsage.rss + memUsage.external,
    });

    console.log(
      `[DOWNLOAD MANAGER] Started: ${downloadId} (${this.activeDownloads.size}/${this.maxConcurrentDownloads} active)`
    );
  }

  public endDownload(downloadId: string): void {
    const downloadInfo = this.activeDownloads.get(downloadId);
    this.activeDownloads.delete(downloadId);

    if (downloadInfo) {
      const duration = Date.now() - downloadInfo.startTime;
      const memUsage = process.memoryUsage();
      const currentMemory = memUsage.rss + memUsage.external;
      const memoryDiff = currentMemory - downloadInfo.memoryAtStart;

      console.log(
        `[DOWNLOAD MANAGER] Ended: ${downloadId} (Duration: ${(duration / 1000).toFixed(1)}s, Memory delta: ${(memoryDiff / 1024 / 1024).toFixed(1)}MB)`
      );

      if (memoryDiff > 100 * 1024 * 1024 && global.gc) {
        setImmediate(() => {
          global.gc!();
          console.log(`[DOWNLOAD MANAGER] Forced GC after download ${downloadId}`);
        });
      }
    }

    this.processQueue();
  }

  private processQueue(): void {
    if (this.downloadQueue.length === 0 || !this.canStartImmediately()) {
      return;
    }

    const nextDownload = this.downloadQueue.shift();
    if (!nextDownload) {
      return;
    }

    console.log(
      `[DOWNLOAD MANAGER] Processing queue: ${nextDownload.downloadId} (${this.downloadQueue.length} remaining)`
    );

    if (nextDownload.metadata?.fileName && nextDownload.metadata?.fileSize) {
      const sizeMB = (nextDownload.metadata.fileSize / (1024 * 1024)).toFixed(1);
      console.log(`[DOWNLOAD MANAGER] Starting queued file: ${nextDownload.metadata.fileName} (${sizeMB}MB)`);
    }

    nextDownload.resolve();
  }

  public getActiveDownloadsCount(): number {
    return this.activeDownloads.size;
  }

  private getCurrentMemoryUsage(): number {
    const usage = process.memoryUsage();
    return (usage.rss + usage.external) / (1024 * 1024);
  }

  public getCurrentMemoryUsageMB(): number {
    return this.getCurrentMemoryUsage();
  }

  public getQueueStatus(): QueueStatus {
    return {
      queueLength: this.downloadQueue.length,
      maxQueueSize: this.maxQueueSize,
      activeDownloads: this.activeDownloads.size,
      maxConcurrent: this.maxConcurrentDownloads,
      queuedDownloads: this.downloadQueue.map((download, index) => ({
        downloadId: download.downloadId,
        position: index + 1,
        waitTime: Date.now() - download.queuedAt,
        fileName: download.metadata?.fileName,
        fileSize: download.metadata?.fileSize,
      })),
    };
  }

  public cancelQueuedDownload(downloadId: string): boolean {
    const index = this.downloadQueue.findIndex((item) => item.downloadId === downloadId);

    if (index === -1) {
      return false;
    }

    const canceledDownload = this.downloadQueue.splice(index, 1)[0];
    canceledDownload.reject(new Error(`Download ${downloadId} was cancelled`));

    console.log(`[DOWNLOAD MANAGER] Cancelled queued download: ${downloadId} (was at position ${index + 1})`);
    return true;
  }

  private cleanupStaleDownloads(): void {
    const now = Date.now();
    const staleThreshold = 10 * 60 * 1000; // 10 minutes
    const queueStaleThreshold = 30 * 60 * 1000;

    for (const [downloadId, info] of this.activeDownloads.entries()) {
      if (now - info.startTime > staleThreshold) {
        console.warn(`[DOWNLOAD MANAGER] Cleaning up stale active download: ${downloadId}`);
        this.activeDownloads.delete(downloadId);
      }
    }

    const initialQueueLength = this.downloadQueue.length;
    this.downloadQueue = this.downloadQueue.filter((download) => {
      if (now - download.queuedAt > queueStaleThreshold) {
        console.warn(`[DOWNLOAD MANAGER] Cleaning up stale queued download: ${download.downloadId}`);
        download.reject(new Error(`Download ${download.downloadId} timed out in queue`));
        return false;
      }
      return true;
    });

    if (this.downloadQueue.length < initialQueueLength) {
      console.log(
        `[DOWNLOAD MANAGER] Cleaned up ${initialQueueLength - this.downloadQueue.length} stale queued downloads`
      );
    }

    this.processQueue();
  }

  public shouldThrottleStream(): boolean {
    const currentMemoryMB = this.getCurrentMemoryUsageMB();
    return currentMemoryMB > this.memoryThresholdMB * 0.8;
  }

  public getThrottleDelay(): number {
    const currentMemoryMB = this.getCurrentMemoryUsageMB();
    const thresholdRatio = currentMemoryMB / this.memoryThresholdMB;

    if (thresholdRatio > 0.9) return 200;
    if (thresholdRatio > 0.8) return 100;
    return 50;
  }

  public destroy(): void {
    if (this.cleanupInterval) {
      clearInterval(this.cleanupInterval);
    }

    this.downloadQueue.forEach((download) => {
      download.reject(new Error("Download manager is shutting down"));
    });

    this.activeDownloads.clear();
    this.downloadQueue = [];
    console.log("[DOWNLOAD MANAGER] Shutdown completed");
  }

  public clearQueue(): number {
    const clearedCount = this.downloadQueue.length;

    this.downloadQueue.forEach((download) => {
      download.reject(new Error("Queue was cleared by administrator"));
    });

    this.downloadQueue = [];
    console.log(`[DOWNLOAD MANAGER] Cleared queue: ${clearedCount} downloads cancelled`);
    return clearedCount;
  }
}
