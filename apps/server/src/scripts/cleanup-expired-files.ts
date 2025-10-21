import { isS3Enabled } from "../config/storage.config";
import { FilesystemStorageProvider } from "../providers/filesystem-storage.provider";
import { S3StorageProvider } from "../providers/s3-storage.provider";
import { prisma } from "../shared/prisma";
import { StorageProvider } from "../types/storage";

/**
 * Script to automatically delete expired files
 * This script should be run periodically (e.g., via cron job)
 */
async function cleanupExpiredFiles() {
  console.log("🧹 Starting expired files cleanup...");
  console.log(`📦 Storage mode: ${isS3Enabled ? "S3" : "Filesystem"}`);

  let storageProvider: StorageProvider;
  if (isS3Enabled) {
    storageProvider = new S3StorageProvider();
  } else {
    storageProvider = FilesystemStorageProvider.getInstance();
  }

  // Get all expired files
  const now = new Date();
  const expiredFiles = await prisma.file.findMany({
    where: {
      expiration: {
        lte: now,
      },
    },
    select: {
      id: true,
      name: true,
      objectName: true,
      userId: true,
      size: true,
      expiration: true,
    },
  });

  console.log(`📊 Found ${expiredFiles.length} expired files`);

  if (expiredFiles.length === 0) {
    console.log("\n✨ No expired files found!");
    return {
      deletedCount: 0,
      failedCount: 0,
      totalSize: 0,
    };
  }

  console.log(`\n🗑️  Expired files to be deleted:`);
  expiredFiles.forEach((file) => {
    const sizeMB = Number(file.size) / (1024 * 1024);
    console.log(`  - ${file.name} (${sizeMB.toFixed(2)} MB) - Expired: ${file.expiration?.toISOString()}`);
  });

  // Ask for confirmation (if running interactively)
  const shouldDelete = process.argv.includes("--confirm");

  if (!shouldDelete) {
    console.log(`\n⚠️  Dry run mode. To actually delete expired files, run with --confirm flag:`);
    console.log(`   pnpm cleanup:expired-files:confirm`);
    return {
      deletedCount: 0,
      failedCount: 0,
      totalSize: 0,
      dryRun: true,
    };
  }

  console.log(`\n🗑️  Deleting expired files...`);

  let deletedCount = 0;
  let failedCount = 0;
  let totalSize = BigInt(0);

  for (const file of expiredFiles) {
    try {
      // Delete from storage first
      await storageProvider.deleteObject(file.objectName);

      // Then delete from database
      await prisma.file.delete({
        where: { id: file.id },
      });

      deletedCount++;
      totalSize += file.size;
      console.log(`  ✓ Deleted: ${file.name}`);
    } catch (error) {
      failedCount++;
      console.error(`  ✗ Failed to delete ${file.name}:`, error);
    }
  }

  const totalSizeMB = Number(totalSize) / (1024 * 1024);

  console.log(`\n✅ Cleanup complete!`);
  console.log(`   Deleted: ${deletedCount} files (${totalSizeMB.toFixed(2)} MB)`);
  if (failedCount > 0) {
    console.log(`   Failed: ${failedCount} files`);
  }

  return {
    deletedCount,
    failedCount,
    totalSize: totalSizeMB,
  };
}

// Run the cleanup
cleanupExpiredFiles()
  .then((result) => {
    console.log("\n✨ Script completed successfully");
    if (result.dryRun) {
      process.exit(0);
    }
    process.exit(result.failedCount > 0 ? 1 : 0);
  })
  .catch((error) => {
    console.error("\n❌ Script failed:", error);
    process.exit(1);
  });
