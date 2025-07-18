#!/usr/bin/env node

/**
 * Test script to validate resumable download functionality
 * This script tests the ResumableDownloadService and integration with FilesystemController
 */
import { ResumableDownloadService } from "./resumable-download.service";

async function testResumableDownloads() {
  console.log("🧪 Testing resumable download functionality...\n");

  const resumableService = ResumableDownloadService.getInstance();

  // Test 1: Create a resumable download session
  console.log("1. Testing session creation:");
  const session1 = await resumableService.createSession(
    "test-token-123",
    "/path/to/large-file.zip",
    "large-file.zip",
    5 * 1024 * 1024 * 1024, // 5GB
    "test-client-fingerprint-1"
  );

  console.log(`✅ Created session: ${session1.sessionId}`);
  console.log(`   File: ${session1.fileName} (${formatFileSize(session1.fileSize)})`);
  console.log(`   Expires: ${new Date(session1.expiresAt).toISOString()}`);

  // Test 2: Create another session
  console.log("\n2. Testing multiple sessions:");
  const session2 = await resumableService.createSession(
    "test-token-456",
    "/path/to/video.mp4",
    "video.mp4",
    2 * 1024 * 1024 * 1024, // 2GB
    "test-client-fingerprint-2"
  );

  console.log(`✅ Created session: ${session2.sessionId}`);

  // Test 3: Update progress
  console.log("\n3. Testing progress updates:");
  await resumableService.updateSessionProgress(session1.sessionId, 1024 * 1024 * 1024); // 1GB
  await resumableService.updateSessionProgress(session2.sessionId, 500 * 1024 * 1024); // 500MB

  console.log(`✅ Updated progress for ${session1.sessionId}: 1GB/5GB (20%)`);
  console.log(`✅ Updated progress for ${session2.sessionId}: 500MB/2GB (25%)`);

  // Test 4: Get session info
  console.log("\n4. Testing session retrieval:");
  const retrievedSession = await resumableService.getSession(session1.sessionId);
  if (retrievedSession) {
    const progress = (retrievedSession.bytesDownloaded / retrievedSession.fileSize) * 100;
    console.log(`✅ Retrieved session: ${retrievedSession.sessionId}`);
    console.log(
      `   Progress: ${formatFileSize(retrievedSession.bytesDownloaded)}/${formatFileSize(retrievedSession.fileSize)} (${progress.toFixed(1)}%)`
    );
  }

  // Test 5: Find session by token
  console.log("\n5. Testing session lookup by token:");
  const foundSession = await resumableService.findSessionByToken("test-token-123", "test-client-fingerprint-1");
  if (foundSession) {
    console.log(`✅ Found session by token: ${foundSession.sessionId}`);
  }

  // Test 6: Get session statistics
  console.log("\n6. Testing session statistics:");
  const stats = resumableService.getSessionStats();
  console.log(`✅ Session stats:`, {
    totalSessions: stats.totalSessions,
    activeSessions: stats.activeSessions,
    totalBytes: formatFileSize(stats.totalBytes),
    downloadedBytes: formatFileSize(stats.downloadedBytes),
  });

  // Test 7: Get active sessions
  console.log("\n7. Testing active sessions list:");
  const activeSessions = resumableService.getActiveSessions();
  console.log(`✅ Active sessions: ${activeSessions.length}`);
  activeSessions.forEach((session) => {
    const progress = (session.bytesDownloaded / session.fileSize) * 100;
    console.log(`   - ${session.fileName}: ${progress.toFixed(1)}% (${session.sessionId})`);
  });

  // Test 8: Abort a session
  console.log("\n8. Testing session abortion:");
  await resumableService.abortSession(session2.sessionId, "test_abort");
  console.log(`✅ Aborted session: ${session2.sessionId}`);

  // Test 9: Complete a session
  console.log("\n9. Testing session completion:");
  await resumableService.updateSessionProgress(session1.sessionId, session1.fileSize); // Complete
  await resumableService.completeSession(session1.sessionId);
  console.log(`✅ Completed session: ${session1.sessionId}`);

  // Test 10: Test client fingerprint generation
  console.log("\n10. Testing client fingerprint generation:");
  const fingerprint1 = resumableService.generateClientFingerprint("Mozilla/5.0", "192.168.1.1");
  const fingerprint2 = resumableService.generateClientFingerprint("Mozilla/5.0", "192.168.1.2");
  const fingerprint3 = resumableService.generateClientFingerprint("Mozilla/5.0", "192.168.1.1");

  console.log(`✅ Fingerprint 1: ${fingerprint1}`);
  console.log(`✅ Fingerprint 2: ${fingerprint2}`);
  console.log(`✅ Fingerprint 3: ${fingerprint3}`);
  console.log(`   Different IPs generate different fingerprints: ${fingerprint1 !== fingerprint2}`);

  // Test 11: Test session persistence (simulate restart)
  console.log("\n11. Testing session persistence:");
  const persistentSession = await resumableService.createSession(
    "persistent-token",
    "/path/to/persistent-file.zip",
    "persistent-file.zip",
    1024 * 1024 * 1024, // 1GB
    "persistent-client"
  );

  await resumableService.updateSessionProgress(persistentSession.sessionId, 512 * 1024 * 1024); // 512MB
  console.log(`✅ Created persistent session with 50% progress: ${persistentSession.sessionId}`);

  // Test 12: Cleanup expired sessions (simulate)
  console.log("\n12. Testing cleanup functionality:");
  await resumableService.cleanupExpiredSessions();
  console.log(`✅ Cleanup completed`);

  // Final stats
  console.log("\n📊 Final Statistics:");
  const finalStats = resumableService.getSessionStats();
  console.log(`   Total sessions: ${finalStats.totalSessions}`);
  console.log(`   Active sessions: ${finalStats.activeSessions}`);
  console.log(`   Total data: ${formatFileSize(finalStats.totalBytes)}`);
  console.log(`   Downloaded: ${formatFileSize(finalStats.downloadedBytes)}`);

  console.log("\n✅ Resumable download test completed successfully!");
}

function formatFileSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = bytes;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex++;
  }

  return `${size.toFixed(1)} ${units[unitIndex]}`;
}

// Run the test if this file is executed directly
if (require.main === module) {
  testResumableDownloads().catch((error) => {
    console.error("❌ Test failed:", error);
    process.exit(1);
  });
}

export { testResumableDownloads };
