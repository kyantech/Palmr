#!/usr/bin/env node

/**
 * Test script to validate comprehensive logging functionality
 * This script tests the DownloadManager and PeriodicCleanupService logging
 */
import { DownloadManager } from "./download-manager";
import { PeriodicCleanupService } from "./periodic-cleanup.service";

async function testComprehensiveLogging() {
  console.log("🧪 Testing comprehensive logging functionality...\n");

  // Test DownloadManager logging
  console.log("1. Testing DownloadManager logging:");
  const downloadManager = DownloadManager.getInstance();

  // Start a test download
  const downloadId1 = downloadManager.startDownload("test-file-1.zip", "test-file-1.zip", 100 * 1024 * 1024); // 100MB
  const downloadId2 = downloadManager.startDownload("test-file-2.pdf", "test-file-2.pdf", 50 * 1024 * 1024); // 50MB

  // Simulate some progress
  downloadManager.updateBytesTransferred(downloadId1, 25 * 1024 * 1024); // 25MB transferred
  downloadManager.updateBytesTransferred(downloadId2, 10 * 1024 * 1024); // 10MB transferred

  // Wait a moment to simulate download time
  await new Promise((resolve) => setTimeout(resolve, 1000));

  // Complete one download
  console.log("\n2. Testing download completion logging:");
  downloadManager.completeDownload(downloadId1);

  // Wait a moment
  await new Promise((resolve) => setTimeout(resolve, 500));

  // Abort the other download
  console.log("\n3. Testing download abortion logging:");
  downloadManager.abortDownload(downloadId2, "client_disconnected", new Error("Client disconnected unexpectedly"));

  // Test memory usage logging
  console.log("\n4. Testing memory usage logging:");
  downloadManager.logMemoryUsageForActiveDownloads();

  // Test memory usage stats
  console.log("\n5. Testing memory usage stats:");
  const stats = downloadManager.getMemoryUsageStats();
  console.log("Memory stats:", {
    activeDownloadCount: stats.activeDownloadCount,
    totalFileSize: stats.totalFileSize,
    memoryUsageMB: {
      rss: Math.round(stats.memoryUsage.rss / 1024 / 1024),
      heapUsed: Math.round(stats.memoryUsage.heapUsed / 1024 / 1024),
      heapTotal: Math.round(stats.memoryUsage.heapTotal / 1024 / 1024),
    },
  });

  // Test PeriodicCleanupService
  console.log("\n6. Testing PeriodicCleanupService:");
  const cleanupService = PeriodicCleanupService.getInstance();

  // Test immediate cleanup
  cleanupService.performImmediateCleanup();

  // Test service status
  const serviceStatus = cleanupService.getServiceStatus();
  console.log("Service status:", {
    isRunning: serviceStatus.isRunning,
    activeDownloadCount: serviceStatus.activeDownloadCount,
    uptimeSeconds: Math.round(serviceStatus.uptime),
  });

  // Start the service briefly to test it
  console.log("\n7. Testing periodic service start/stop:");
  cleanupService.start(30000, 15000); // 30s cleanup, 15s memory monitor

  await new Promise((resolve) => setTimeout(resolve, 2000));

  cleanupService.stop();

  console.log("\n✅ Comprehensive logging test completed successfully!");
}

// Run the test if this file is executed directly
if (require.main === module) {
  testComprehensiveLogging().catch((error) => {
    console.error("❌ Test failed:", error);
    process.exit(1);
  });
}

export { testComprehensiveLogging };
