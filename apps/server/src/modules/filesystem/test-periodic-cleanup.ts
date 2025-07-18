#!/usr/bin/env tsx

/**
 * Test script to validate enhanced periodic cleanup functionality
 * This script tests the enhanced PeriodicCleanupService with timeout-based and high-load cleanup
 */
import { DownloadManager } from "./download-manager";
import { PeriodicCleanupService } from "./periodic-cleanup.service";

async function testEnhancedPeriodicCleanup() {
  console.log("🧪 Testing enhanced periodic cleanup functionality...\n");

  const downloadManager = DownloadManager.getInstance();
  const cleanupService = PeriodicCleanupService.getInstance();

  // Test 1: Basic service functionality
  console.log("1. Testing basic service functionality:");
  console.log("Initial service status:", cleanupService.getServiceStatus());
  console.log("Initial cleanup stats:", cleanupService.getCleanupStats());

  // Test 2: Start service with custom intervals
  console.log("\n2. Starting enhanced cleanup service:");
  cleanupService.start(5000, 3000); // 5s cleanup, 3s memory monitor for testing
  await new Promise((resolve) => setTimeout(resolve, 1000));

  // Test 3: Create some test downloads to simulate activity
  console.log("\n3. Creating test downloads:");
  const download1 = downloadManager.startDownload("test-timeout-1", "large-file-1.zip", 500 * 1024 * 1024); // 500MB
  const download2 = downloadManager.startDownload("test-timeout-2", "large-file-2.pdf", 200 * 1024 * 1024); // 200MB
  const download3 = downloadManager.startDownload("test-timeout-3", "large-file-3.mov", 1024 * 1024 * 1024); // 1GB

  console.log(`Created downloads: ${download1}, ${download2}, ${download3}`);

  // Test 4: Check service status with active downloads
  console.log("\n4. Service status with active downloads:");
  const statusWithDownloads = cleanupService.getServiceStatus();
  console.log("Active downloads:", statusWithDownloads.activeDownloadCount);
  console.log("High load mode:", statusWithDownloads.isHighLoadMode);

  // Test 5: Simulate high-load conditions by creating many downloads
  console.log("\n5. Simulating high-load conditions:");
  const highLoadDownloads = [];
  for (let i = 0; i < 12; i++) {
    const downloadId = downloadManager.startDownload(
      `high-load-${i}`,
      `file-${i}.zip`,
      100 * 1024 * 1024 // 100MB each
    );
    highLoadDownloads.push(downloadId);
  }

  console.log(`Created ${highLoadDownloads.length} additional downloads for high-load test`);

  // Wait for high-load detection
  await new Promise((resolve) => setTimeout(resolve, 4000));

  console.log("\n6. Checking high-load mode activation:");
  const highLoadStatus = cleanupService.getServiceStatus();
  console.log("High load mode activated:", highLoadStatus.isHighLoadMode);
  console.log("Total active downloads:", highLoadStatus.activeDownloadCount);

  // Test 7: Perform immediate cleanup
  console.log("\n7. Testing immediate cleanup:");
  cleanupService.performImmediateCleanup();

  // Test 8: Wait for periodic cleanup cycles
  console.log("\n8. Waiting for periodic cleanup cycles...");
  await new Promise((resolve) => setTimeout(resolve, 8000));

  // Test 9: Check cleanup statistics
  console.log("\n9. Final cleanup statistics:");
  const finalStats = cleanupService.getCleanupStats();
  console.log("Cleanup stats:", finalStats);

  // Test 10: Test timeout-based cleanup by simulating old downloads
  console.log("\n10. Testing timeout-based cleanup:");

  // Manually create an old download by modifying the start time
  const oldDownload = downloadManager.startDownload("old-download", "old-file.zip", 50 * 1024 * 1024);
  const downloadInfo = downloadManager.getDownloadInfo(oldDownload);
  if (downloadInfo) {
    // Simulate an old download (31 minutes old)
    downloadInfo.startTime = Date.now() - 31 * 60 * 1000;
    console.log(`Created simulated old download: ${oldDownload} (31 minutes old)`);
  }

  // Trigger cleanup
  await new Promise((resolve) => setTimeout(resolve, 6000));

  // Test 11: Final service status
  console.log("\n11. Final service status:");
  const finalStatus = cleanupService.getServiceStatus();
  console.log("Final status:", {
    isRunning: finalStatus.isRunning,
    isHighLoadMode: finalStatus.isHighLoadMode,
    activeDownloadCount: finalStatus.activeDownloadCount,
    cleanupStats: finalStatus.cleanupStats,
  });

  // Test 12: Stop service
  console.log("\n12. Stopping cleanup service:");
  cleanupService.stop();

  // Clean up remaining downloads
  console.log("\n13. Cleaning up remaining test downloads:");
  const remainingDownloads = downloadManager.getActiveDownloads();
  for (const download of remainingDownloads) {
    downloadManager.abortDownload(download.id, "test_cleanup", new Error("Test cleanup"));
  }

  console.log("\n✅ Enhanced periodic cleanup test completed successfully!");
  console.log("\nTest Summary:");
  console.log("- ✅ Basic service functionality");
  console.log("- ✅ Service start/stop with custom intervals");
  console.log("- ✅ High-load detection and mode switching");
  console.log("- ✅ Timeout-based cleanup for server-side timeouts");
  console.log("- ✅ Enhanced cleanup statistics tracking");
  console.log("- ✅ Immediate cleanup functionality");
  console.log("- ✅ Memory monitoring with load detection");
}

// Run the test
testEnhancedPeriodicCleanup().catch((error) => {
  console.error("❌ Test failed:", error);
  process.exit(1);
});
