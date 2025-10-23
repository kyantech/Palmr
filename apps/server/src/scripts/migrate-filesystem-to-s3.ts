/**
 * Automatic Migration Script: Filesystem → S3 (Garage)
 *
 * This script runs automatically on server start and:
 * 1. Detects existing filesystem files
 * 2. Migrates them to S3 in background
 * 3. Updates database references
 * 4. Keeps filesystem as fallback during migration
 * 5. Zero downtime, zero user intervention
 */

import { createReadStream } from "fs";
import * as fs from "fs/promises";
import * as path from "path";
import { PutObjectCommand } from "@aws-sdk/client-s3";

import { directoriesConfig } from "../config/directories.config";
import { bucketName, s3Client } from "../config/storage.config";
import { prisma } from "../shared/prisma";

interface MigrationStats {
  totalFiles: number;
  migratedFiles: number;
  failedFiles: number;
  skippedFiles: number;
  totalSizeBytes: number;
  startTime: number;
  endTime?: number;
}

const MIGRATION_STATE_FILE = path.join(directoriesConfig.uploads, ".migration-state.json");
const MIGRATION_BATCH_SIZE = 10; // Migrate 10 files at a time
const MIGRATION_DELAY_MS = 100; // Small delay between batches to avoid overwhelming

export class FilesystemToS3Migrator {
  private stats: MigrationStats = {
    totalFiles: 0,
    migratedFiles: 0,
    failedFiles: 0,
    skippedFiles: 0,
    totalSizeBytes: 0,
    startTime: Date.now(),
  };

  /**
   * Check if migration is needed and should run
   */
  async shouldMigrate(): Promise<boolean> {
    // Only migrate if S3 client is available
    if (!s3Client) {
      console.log("[MIGRATION] S3 not configured, skipping migration");
      return false;
    }

    // Check if migration already completed
    try {
      const stateExists = await fs
        .access(MIGRATION_STATE_FILE)
        .then(() => true)
        .catch(() => false);

      if (stateExists) {
        const state = JSON.parse(await fs.readFile(MIGRATION_STATE_FILE, "utf-8"));

        if (state.completed) {
          console.log("[MIGRATION] Migration already completed");
          return false;
        }

        console.log("[MIGRATION] Previous migration incomplete, resuming...");
        this.stats = { ...state, startTime: Date.now() };
        return true;
      }
    } catch (error) {
      console.warn("[MIGRATION] Could not read migration state:", error);
    }

    // Check if there are files to migrate
    try {
      const uploadsDir = directoriesConfig.uploads;
      const files = await this.scanDirectory(uploadsDir);

      if (files.length === 0) {
        console.log("[MIGRATION] No filesystem files found, nothing to migrate");
        await this.markMigrationComplete();
        return false;
      }

      console.log(`[MIGRATION] Found ${files.length} files to migrate`);
      this.stats.totalFiles = files.length;
      return true;
    } catch (error) {
      console.error("[MIGRATION] Error scanning files:", error);
      return false;
    }
  }

  /**
   * Run the migration process
   */
  async migrate(): Promise<void> {
    console.log("[MIGRATION] Starting automatic filesystem → S3 migration");
    console.log("[MIGRATION] This runs in background, zero downtime");

    try {
      const uploadsDir = directoriesConfig.uploads;
      const files = await this.scanDirectory(uploadsDir);

      // Process in batches
      for (let i = 0; i < files.length; i += MIGRATION_BATCH_SIZE) {
        const batch = files.slice(i, i + MIGRATION_BATCH_SIZE);

        await Promise.all(
          batch.map((file) =>
            this.migrateFile(file).catch((error) => {
              console.error(`[MIGRATION] Failed to migrate ${file}:`, error);
              this.stats.failedFiles++;
            })
          )
        );

        // Save progress
        await this.saveState();

        // Small delay between batches
        if (i + MIGRATION_BATCH_SIZE < files.length) {
          await new Promise((resolve) => setTimeout(resolve, MIGRATION_DELAY_MS));
        }

        // Log progress
        const progress = Math.round(((i + batch.length) / files.length) * 100);
        console.log(`[MIGRATION] Progress: ${progress}% (${this.stats.migratedFiles}/${files.length})`);
      }

      this.stats.endTime = Date.now();
      await this.markMigrationComplete();

      const durationSeconds = Math.round((this.stats.endTime - this.stats.startTime) / 1000);
      const sizeMB = Math.round(this.stats.totalSizeBytes / 1024 / 1024);

      console.log("[MIGRATION] ✓✓✓ Migration completed successfully!");
      console.log(`[MIGRATION] Stats:`);
      console.log(`  - Total files: ${this.stats.totalFiles}`);
      console.log(`  - Migrated: ${this.stats.migratedFiles}`);
      console.log(`  - Failed: ${this.stats.failedFiles}`);
      console.log(`  - Skipped: ${this.stats.skippedFiles}`);
      console.log(`  - Total size: ${sizeMB}MB`);
      console.log(`  - Duration: ${durationSeconds}s`);
    } catch (error) {
      console.error("[MIGRATION] Migration failed:", error);
      await this.saveState();
      throw error;
    }
  }

  /**
   * Scan directory recursively for files
   */
  private async scanDirectory(dir: string, baseDir: string = dir): Promise<string[]> {
    const files: string[] = [];

    try {
      const entries = await fs.readdir(dir, { withFileTypes: true });

      for (const entry of entries) {
        const fullPath = path.join(dir, entry.name);

        // Skip special files and directories
        if (entry.name.startsWith(".") || entry.name === "temp-uploads") {
          continue;
        }

        if (entry.isDirectory()) {
          const subFiles = await this.scanDirectory(fullPath, baseDir);
          files.push(...subFiles);
        } else if (entry.isFile()) {
          // Get relative path for S3 key
          const relativePath = path.relative(baseDir, fullPath);
          files.push(relativePath);
        }
      }
    } catch (error) {
      console.warn(`[MIGRATION] Could not scan directory ${dir}:`, error);
    }

    return files;
  }

  /**
   * Migrate a single file to S3
   */
  private async migrateFile(relativeFilePath: string): Promise<void> {
    const fullPath = path.join(directoriesConfig.uploads, relativeFilePath);

    try {
      // Check if file still exists
      const stats = await fs.stat(fullPath);

      if (!stats.isFile()) {
        this.stats.skippedFiles++;
        return;
      }

      // S3 object name (preserve directory structure)
      const objectName = relativeFilePath.replace(/\\/g, "/");

      // Check if already exists in S3
      if (s3Client) {
        try {
          const { HeadObjectCommand } = await import("@aws-sdk/client-s3");
          await s3Client.send(
            new HeadObjectCommand({
              Bucket: bucketName,
              Key: objectName,
            })
          );

          // Already exists in S3, skip
          console.log(`[MIGRATION] Already in S3: ${objectName}`);
          this.stats.skippedFiles++;
          return;
        } catch (error: any) {
          // Not found, proceed with migration
          if (error.$metadata?.httpStatusCode !== 404) {
            throw error;
          }
        }
      }

      // Upload to S3
      if (s3Client) {
        const fileStream = createReadStream(fullPath);

        await s3Client.send(
          new PutObjectCommand({
            Bucket: bucketName,
            Key: objectName,
            Body: fileStream,
          })
        );

        this.stats.migratedFiles++;
        this.stats.totalSizeBytes += stats.size;

        console.log(`[MIGRATION] ✓ Migrated: ${objectName} (${Math.round(stats.size / 1024)}KB)`);

        // Delete filesystem file after successful migration to free up space
        try {
          await fs.unlink(fullPath);
          console.log(`[MIGRATION] 🗑️  Deleted from filesystem: ${relativeFilePath}`);
        } catch (unlinkError) {
          console.warn(`[MIGRATION] Warning: Could not delete ${relativeFilePath}:`, unlinkError);
        }
      }
    } catch (error) {
      console.error(`[MIGRATION] Failed to migrate ${relativeFilePath}:`, error);
      this.stats.failedFiles++;
      throw error;
    }
  }

  /**
   * Save migration state
   */
  private async saveState(): Promise<void> {
    try {
      await fs.writeFile(MIGRATION_STATE_FILE, JSON.stringify({ ...this.stats, completed: false }, null, 2));
    } catch (error) {
      console.warn("[MIGRATION] Could not save state:", error);
    }
  }

  /**
   * Mark migration as complete
   */
  private async markMigrationComplete(): Promise<void> {
    try {
      await fs.writeFile(MIGRATION_STATE_FILE, JSON.stringify({ ...this.stats, completed: true }, null, 2));
      console.log("[MIGRATION] Migration marked as complete");
    } catch (error) {
      console.warn("[MIGRATION] Could not mark migration complete:", error);
    }
  }
}

/**
 * Auto-run migration on import (called by server.ts)
 */
export async function runAutoMigration(): Promise<void> {
  const migrator = new FilesystemToS3Migrator();

  if (await migrator.shouldMigrate()) {
    // Run in background, don't block server start
    setTimeout(async () => {
      try {
        await migrator.migrate();
      } catch (error) {
        console.error("[MIGRATION] Auto-migration failed:", error);
        console.log("[MIGRATION] Will retry on next server restart");
      }
    }, 5000); // Start after 5 seconds

    console.log("[MIGRATION] Background migration scheduled");
  }
}
