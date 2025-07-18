import { timeoutConfig } from "../../config/timeout.config";
import { DownloadManager } from "./download-manager";

/**
 * Cleanup statistics interface
 */
interface CleanupStats {
  totalCleanupsPerformed: number;
  orphanedDownloadsFound: number;
  timeoutBasedCleanups: number;
  highLoadCleanups: number;
}

/**
 * Service for periodic cleanup and monitoring of download operations
 * Handles memory monitoring and orphaned download cleanup
 */
export class PeriodicCleanupService {
  private static instance: PeriodicCleanupService;
  private downloadManager: DownloadManager;
  private cleanupInterval?: NodeJS.Timeout;
  private memoryMonitorInterval?: NodeJS.Timeout;
  private highLoadCleanupInterval?: NodeJS.Timeout;
  private isRunning = false;
  private isHighLoadMode = false;
  private lastCleanupTime = 0;
  private cleanupStats = {
    totalCleanupsPerformed: 0,
    orphanedDownloadsFound: 0,
    timeoutBasedCleanups: 0,
    highLoadCleanups: 0,
  };

  private constructor() {
    this.downloadManager = DownloadManager.getInstance();
  }

  /**
   * Get the singleton instance of PeriodicCleanupService
   */
  public static getInstance(): PeriodicCleanupService {
    if (!PeriodicCleanupService.instance) {
      PeriodicCleanupService.instance = new PeriodicCleanupService();
    }
    return PeriodicCleanupService.instance;
  }

  /**
   * Start the periodic cleanup and monitoring services
   * @param cleanupIntervalMs Interval for orphaned download cleanup (default: 10 minutes)
   * @param memoryMonitorIntervalMs Interval for memory usage monitoring (default: 5 minutes)
   */
  start(cleanupIntervalMs: number = 10 * 60 * 1000, memoryMonitorIntervalMs: number = 5 * 60 * 1000): void {
    if (this.isRunning) {
      console.log("⚠️ Periodic cleanup service is already running");
      return;
    }

    console.log(
      `🚀 Starting periodic cleanup service (cleanup: ${this.formatDuration(cleanupIntervalMs)}, memory monitor: ${this.formatDuration(memoryMonitorIntervalMs)})`
    );

    // Start orphaned download cleanup with enhanced timeout-based cleanup
    this.cleanupInterval = setInterval(() => {
      try {
        this.performEnhancedCleanup();
        this.cleanupStats.totalCleanupsPerformed++;
      } catch (error) {
        console.error("❌ Error during periodic cleanup:", error);
      }
    }, cleanupIntervalMs);

    // Start memory usage monitoring with high-load detection
    this.memoryMonitorInterval = setInterval(() => {
      try {
        this.monitorMemoryUsageWithLoadDetection();
      } catch (error) {
        console.error("❌ Error during memory monitoring:", error);
      }
    }, memoryMonitorIntervalMs);

    // Start high-load cleanup (more frequent cleanup when under high load)
    this.highLoadCleanupInterval = setInterval(
      () => {
        try {
          if (this.isHighLoadMode) {
            this.performHighLoadCleanup();
            this.cleanupStats.highLoadCleanups++;
          }
        } catch (error) {
          console.error("❌ Error during high-load cleanup:", error);
        }
      },
      Math.min(cleanupIntervalMs / 3, 2 * 60 * 1000)
    ); // Every 2 minutes or 1/3 of cleanup interval

    this.isRunning = true;
    this.lastCleanupTime = Date.now();

    // Log initial memory usage and cleanup stats
    this.monitorMemoryUsage();
    console.log(`🧹 Enhanced cleanup service started with timeout-based and high-load cleanup`);
  }

  /**
   * Stop the periodic cleanup and monitoring services
   */
  stop(): void {
    if (!this.isRunning) {
      console.log("⚠️ Periodic cleanup service is not running");
      return;
    }

    console.log("🛑 Stopping periodic cleanup service");

    if (this.cleanupInterval) {
      clearInterval(this.cleanupInterval);
      this.cleanupInterval = undefined;
    }

    if (this.memoryMonitorInterval) {
      clearInterval(this.memoryMonitorInterval);
      this.memoryMonitorInterval = undefined;
    }

    if (this.highLoadCleanupInterval) {
      clearInterval(this.highLoadCleanupInterval);
      this.highLoadCleanupInterval = undefined;
    }

    this.isRunning = false;
    this.isHighLoadMode = false;

    // Log final cleanup statistics
    console.log(
      `📊 [CLEANUP_STATS] Total cleanups: ${this.cleanupStats.totalCleanupsPerformed}, Orphaned found: ${this.cleanupStats.orphanedDownloadsFound}, Timeout cleanups: ${this.cleanupStats.timeoutBasedCleanups}, High-load cleanups: ${this.cleanupStats.highLoadCleanups}`
    );
  }

  /**
   * Check if the service is currently running
   */
  isServiceRunning(): boolean {
    return this.isRunning;
  }

  /**
   * Perform immediate cleanup and memory monitoring
   * This can be called manually for testing or immediate cleanup
   */
  performImmediateCleanup(): void {
    console.log("🧹 Performing immediate cleanup and memory monitoring");

    try {
      this.downloadManager.cleanupOrphanedDownloads();
      this.monitorMemoryUsage();
    } catch (error) {
      console.error("❌ Error during immediate cleanup:", error);
    }
  }

  /**
   * Monitor current memory usage and log statistics
   */
  private monitorMemoryUsage(): void {
    const stats = this.downloadManager.getMemoryUsageStats();
    const memoryMB = {
      rss: Math.round(stats.memoryUsage.rss / 1024 / 1024),
      heapUsed: Math.round(stats.memoryUsage.heapUsed / 1024 / 1024),
      heapTotal: Math.round(stats.memoryUsage.heapTotal / 1024 / 1024),
      external: Math.round(stats.memoryUsage.external / 1024 / 1024),
      arrayBuffers: Math.round(stats.memoryUsage.arrayBuffers / 1024 / 1024),
    };

    // Log memory usage with active download context
    console.log(
      `📊 [MEMORY_MONITOR] RSS=${memoryMB.rss}MB, Heap=${memoryMB.heapUsed}/${memoryMB.heapTotal}MB, External=${memoryMB.external}MB, ArrayBuffers=${memoryMB.arrayBuffers}MB`
    );
    console.log(
      `📊 [DOWNLOAD_STATS] Active downloads: ${stats.activeDownloadCount}, Total file size: ${this.formatFileSize(stats.totalFileSize)}`
    );

    // Log active download IDs if any
    if (stats.activeDownloadCount > 0) {
      console.log(`📊 [ACTIVE_DOWNLOADS] ${stats.activeDownloadIds.join(", ")}`);
    }

    // Check for potential memory issues
    this.checkMemoryThresholds(memoryMB, stats);

    // Log memory usage for active downloads
    this.downloadManager.logMemoryUsageForActiveDownloads();
  }

  /**
   * Check memory usage against thresholds and log warnings
   * @param memoryMB Memory usage in MB
   * @param stats Download statistics
   */
  private checkMemoryThresholds(
    memoryMB: { rss: number; heapUsed: number; heapTotal: number; external: number; arrayBuffers: number },
    stats: { activeDownloadCount: number; totalFileSize: number }
  ): void {
    // Define memory thresholds (these can be made configurable)
    const RSS_WARNING_THRESHOLD = 1024; // 1GB
    const RSS_CRITICAL_THRESHOLD = 2048; // 2GB
    const HEAP_WARNING_THRESHOLD = 512; // 512MB
    const HEAP_CRITICAL_THRESHOLD = 1024; // 1GB

    // Check RSS memory
    if (memoryMB.rss > RSS_CRITICAL_THRESHOLD) {
      console.error(
        `🚨 [MEMORY_CRITICAL] RSS memory usage is critically high: ${memoryMB.rss}MB (threshold: ${RSS_CRITICAL_THRESHOLD}MB)`
      );
      console.error(
        `🚨 [MEMORY_CRITICAL] Active downloads: ${stats.activeDownloadCount}, Total file size: ${this.formatFileSize(stats.totalFileSize)}`
      );
    } else if (memoryMB.rss > RSS_WARNING_THRESHOLD) {
      console.warn(
        `⚠️ [MEMORY_WARNING] RSS memory usage is high: ${memoryMB.rss}MB (threshold: ${RSS_WARNING_THRESHOLD}MB)`
      );
      console.warn(
        `⚠️ [MEMORY_WARNING] Active downloads: ${stats.activeDownloadCount}, Total file size: ${this.formatFileSize(stats.totalFileSize)}`
      );
    }

    // Check heap memory
    if (memoryMB.heapUsed > HEAP_CRITICAL_THRESHOLD) {
      console.error(
        `🚨 [HEAP_CRITICAL] Heap memory usage is critically high: ${memoryMB.heapUsed}MB (threshold: ${HEAP_CRITICAL_THRESHOLD}MB)`
      );
    } else if (memoryMB.heapUsed > HEAP_WARNING_THRESHOLD) {
      console.warn(
        `⚠️ [HEAP_WARNING] Heap memory usage is high: ${memoryMB.heapUsed}MB (threshold: ${HEAP_WARNING_THRESHOLD}MB)`
      );
    }

    // Check for potential memory leaks (high memory with no active downloads)
    if (memoryMB.rss > RSS_WARNING_THRESHOLD && stats.activeDownloadCount === 0) {
      console.warn(
        `⚠️ [POTENTIAL_LEAK] High memory usage (${memoryMB.rss}MB) with no active downloads - potential memory leak`
      );
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
   * Enhanced cleanup that includes timeout-based cleanup for server-side timeouts
   * This addresses requirement 3.2: server-side timeout cleanup
   */
  private performEnhancedCleanup(): void {
    const now = Date.now();
    const activeDownloads = this.downloadManager.getActiveDownloads();

    console.log(`🧹 Starting enhanced cleanup check (${activeDownloads.length} active downloads)`);

    // Standard orphaned download cleanup (existing functionality)
    const orphanedCount = this.downloadManager.cleanupOrphanedDownloads();
    this.cleanupStats.orphanedDownloadsFound += orphanedCount;

    // Enhanced timeout-based cleanup for server-side timeouts
    let timeoutCleanupCount = 0;
    const streamTimeout = timeoutConfig.file.streamTimeout || 30000; // 30 seconds default
    const maxDownloadAge = 30 * 60 * 1000; // 30 minutes max age

    for (const download of activeDownloads) {
      const age = now - download.startTime;
      const timeSinceLastActivity = now - (download.startTime + (download.bytesTransferred || 0) / 1000); // Rough estimate

      // Check for server-side timeout conditions
      let shouldCleanup = false;
      let reason = "";

      // 1. Download exceeded maximum age
      if (age > maxDownloadAge) {
        shouldCleanup = true;
        reason = "max_age_exceeded";
      }
      // 2. Stream appears stalled (no progress for stream timeout period)
      else if (timeSinceLastActivity > streamTimeout * 2) {
        shouldCleanup = true;
        reason = "stream_timeout";
      }
      // 3. Download has been running too long relative to file size (potential stall)
      else if (download.fileSize > 0) {
        const expectedDuration = download.fileSize / (1024 * 1024); // Assume 1MB/s minimum
        if (age > expectedDuration * 1000 * 10) {
          // 10x expected duration
          shouldCleanup = true;
          reason = "performance_timeout";
        }
      }

      if (shouldCleanup) {
        console.log(
          `🧹 [TIMEOUT_CLEANUP] Cleaning up download ${download.id} due to ${reason} (age: ${this.formatDuration(age)})`
        );
        this.downloadManager.abortDownload(
          download.id,
          `server_timeout_${reason}`,
          new Error(`Download timed out due to ${reason}`)
        );
        timeoutCleanupCount++;
        this.cleanupStats.timeoutBasedCleanups++;
      }
    }

    if (timeoutCleanupCount > 0) {
      console.log(`🧹 [TIMEOUT_CLEANUP] Cleaned up ${timeoutCleanupCount} downloads due to server-side timeouts`);
    }

    this.lastCleanupTime = now;
  }

  /**
   * Monitor memory usage with high-load detection
   * This addresses requirement 1.3: cleanup under high load
   */
  private monitorMemoryUsageWithLoadDetection(): void {
    const stats = this.downloadManager.getMemoryUsageStats();
    const memoryMB = {
      rss: Math.round(stats.memoryUsage.rss / 1024 / 1024),
      heapUsed: Math.round(stats.memoryUsage.heapUsed / 1024 / 1024),
      heapTotal: Math.round(stats.memoryUsage.heapTotal / 1024 / 1024),
      external: Math.round(stats.memoryUsage.external / 1024 / 1024),
      arrayBuffers: Math.round(stats.memoryUsage.arrayBuffers / 1024 / 1024),
    };

    // Detect high-load conditions
    const HIGH_LOAD_THRESHOLDS = {
      activeDownloads: 10, // More than 10 concurrent downloads
      memoryRSS: 1024, // More than 1GB RSS
      memoryHeap: 512, // More than 512MB heap
    };

    const wasHighLoad = this.isHighLoadMode;
    this.isHighLoadMode =
      stats.activeDownloadCount > HIGH_LOAD_THRESHOLDS.activeDownloads ||
      memoryMB.rss > HIGH_LOAD_THRESHOLDS.memoryRSS ||
      memoryMB.heapUsed > HIGH_LOAD_THRESHOLDS.memoryHeap;

    // Log high-load mode changes
    if (this.isHighLoadMode && !wasHighLoad) {
      console.warn(
        `⚠️ [HIGH_LOAD] Entering high-load mode: ${stats.activeDownloadCount} downloads, ${memoryMB.rss}MB RSS, ${memoryMB.heapUsed}MB heap`
      );
    } else if (!this.isHighLoadMode && wasHighLoad) {
      console.log(
        `✅ [HIGH_LOAD] Exiting high-load mode: ${stats.activeDownloadCount} downloads, ${memoryMB.rss}MB RSS, ${memoryMB.heapUsed}MB heap`
      );
    }

    // Regular memory monitoring
    this.monitorMemoryUsage();

    // Force garbage collection in high-load mode if available
    if (this.isHighLoadMode && global.gc) {
      try {
        global.gc();
        console.log(`🗑️ [HIGH_LOAD] Forced garbage collection`);
      } catch (error) {
        console.warn(`⚠️ [HIGH_LOAD] Could not force garbage collection:`, error);
      }
    }
  }

  /**
   * Perform more aggressive cleanup during high-load conditions
   * This addresses requirement 1.3: cleanup under high load
   */
  private performHighLoadCleanup(): void {
    const now = Date.now();
    const activeDownloads = this.downloadManager.getActiveDownloads();

    console.log(`🚨 [HIGH_LOAD_CLEANUP] Performing aggressive cleanup (${activeDownloads.length} active downloads)`);

    let cleanedCount = 0;
    const AGGRESSIVE_TIMEOUT = 15 * 60 * 1000; // 15 minutes (more aggressive than normal 30 minutes)

    for (const download of activeDownloads) {
      const age = now - download.startTime;

      // More aggressive cleanup criteria during high load
      if (age > AGGRESSIVE_TIMEOUT) {
        console.log(
          `🚨 [HIGH_LOAD_CLEANUP] Aggressively cleaning up download ${download.id} (age: ${this.formatDuration(age)})`
        );
        this.downloadManager.abortDownload(
          download.id,
          "high_load_cleanup",
          new Error(`Download aborted due to high-load cleanup after ${this.formatDuration(age)}`)
        );
        cleanedCount++;
      }
    }

    if (cleanedCount > 0) {
      console.log(`🚨 [HIGH_LOAD_CLEANUP] Aggressively cleaned up ${cleanedCount} downloads`);
    }

    // Also perform memory monitoring
    this.monitorMemoryUsage();
  }

  /**
   * Get enhanced service status information including cleanup statistics
   */
  getServiceStatus(): {
    isRunning: boolean;
    isHighLoadMode: boolean;
    activeDownloadCount: number;
    memoryUsage: NodeJS.MemoryUsage;
    uptime: number;
    cleanupStats: CleanupStats;
    lastCleanupTime: number;
  } {
    const stats = this.downloadManager.getMemoryUsageStats();

    return {
      isRunning: this.isRunning,
      isHighLoadMode: this.isHighLoadMode,
      activeDownloadCount: stats.activeDownloadCount,
      memoryUsage: stats.memoryUsage,
      uptime: process.uptime(),
      cleanupStats: { ...this.cleanupStats },
      lastCleanupTime: this.lastCleanupTime,
    };
  }

  /**
   * Get cleanup statistics for monitoring
   */
  getCleanupStats(): CleanupStats & {
    isHighLoadMode: boolean;
    lastCleanupAge: number;
  } {
    return {
      ...this.cleanupStats,
      isHighLoadMode: this.isHighLoadMode,
      lastCleanupAge: this.lastCleanupTime > 0 ? Date.now() - this.lastCleanupTime : 0,
    };
  }
}
