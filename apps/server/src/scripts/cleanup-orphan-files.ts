import { S3StorageProvider } from "../providers/s3-storage.provider";
import { prisma } from "../shared/prisma";
import { StorageProvider } from "../types/storage";

/**
 * Script to clean up orphan file records in the database
 * (files that are registered in DB but don't exist in storage)
 */
async function cleanupOrphanFiles() {
  console.log("Starting orphan file cleanup...");
  console.log(`Storage mode: S3 (Garage or External)`);

  // Always use S3 storage provider
  const storageProvider: StorageProvider = new S3StorageProvider();

  // Get all files from database
  const allFiles = await prisma.file.findMany({
    select: {
      id: true,
      name: true,
      objectName: true,
      userId: true,
      size: true,
    },
  });

  console.log(`Found ${allFiles.length} files in database`);

  const orphanFiles: typeof allFiles = [];
  const existingFiles: typeof allFiles = [];

  // Check each file
  for (const file of allFiles) {
    const exists = await storageProvider.fileExists(file.objectName);
    if (!exists) {
      orphanFiles.push(file);
      console.log(`❌ Orphan: ${file.name} (${file.objectName})`);
    } else {
      existingFiles.push(file);
    }
  }

  console.log(`\n📊 Summary:`);
  console.log(`  Total files in DB: ${allFiles.length}`);
  console.log(`  ✅ Files with storage: ${existingFiles.length}`);
  console.log(`  ❌ Orphan files: ${orphanFiles.length}`);

  if (orphanFiles.length === 0) {
    console.log("\n✨ No orphan files found!");
    return;
  }

  console.log(`\n🗑️  Orphan files to be deleted:`);
  orphanFiles.forEach((file) => {
    const sizeMB = Number(file.size) / (1024 * 1024);
    console.log(`  - ${file.name} (${sizeMB.toFixed(2)} MB) - ${file.objectName}`);
  });

  // Ask for confirmation (if running interactively)
  const shouldDelete = process.argv.includes("--confirm");

  if (!shouldDelete) {
    console.log(`\n⚠️  Dry run mode. To actually delete orphan records, run with --confirm flag:`);
    console.log(`   node dist/scripts/cleanup-orphan-files.js --confirm`);
    return;
  }

  console.log(`\n🗑️  Deleting orphan file records...`);

  let deletedCount = 0;
  for (const file of orphanFiles) {
    try {
      await prisma.file.delete({
        where: { id: file.id },
      });
      deletedCount++;
      console.log(`  ✓ Deleted: ${file.name}`);
    } catch (error) {
      console.error(`  ✗ Failed to delete ${file.name}:`, error);
    }
  }

  console.log(`\n✅ Cleanup complete!`);
  console.log(`   Deleted ${deletedCount} orphan file records`);
}

// Run the cleanup
cleanupOrphanFiles()
  .then(() => {
    console.log("\nScript completed successfully");
    process.exit(0);
  })
  .catch((error) => {
    console.error("\n❌ Script failed:", error);
    process.exit(1);
  });
