import { exec } from "child_process";
import fs from "node:fs";
import { promisify } from "util";
import { PrismaClient } from "@prisma/client";

import { IS_RUNNING_IN_CONTAINER } from "../../utils/container-detection";
import { ConfigService } from "../config/service";

const execAsync = promisify(exec);
const prisma = new PrismaClient();

export class StorageService {
  private configService = new ConfigService();

  private _ensureNumber(value: number, fallback: number = 0): number {
    return Number.isNaN(value) || !Number.isFinite(value) || value < 0 ? fallback : value;
  }

  private _safeParseInt(value: string): number {
    const parsed = parseInt(value, 10);
    return Number.isNaN(parsed) ? 0 : parsed;
  }

  private async _tryDiskSpaceCommand(command: string): Promise<{ total: number; available: number } | null> {
    try {
      const { stdout, stderr } = await execAsync(command);

      if (stderr) {
        console.warn(`Command stderr: ${stderr}`);
      }

      let total = 0;
      let available = 0;

      if (process.platform === "win32") {
        const lines = stdout.trim().split("\n").slice(1);
        for (const line of lines) {
          const parts = line.trim().split(/\s+/);
          if (parts.length >= 3) {
            const [, size, freespace] = parts;
            total += this._safeParseInt(size);
            available += this._safeParseInt(freespace);
          }
        }
      } else if (process.platform === "darwin") {
        const lines = stdout.trim().split("\n");
        if (lines.length >= 2) {
          const parts = lines[1].trim().split(/\s+/);
          if (parts.length >= 4) {
            const [, size, , avail] = parts;
            total = this._safeParseInt(size) * 1024;
            available = this._safeParseInt(avail) * 1024;
          }
        }
      } else {
        const lines = stdout.trim().split("\n");
        if (lines.length >= 2) {
          const parts = lines[1].trim().split(/\s+/);
          if (parts.length >= 4) {
            const [, size, , avail] = parts;
            if (command.includes("-B1")) {
              total = this._safeParseInt(size);
              available = this._safeParseInt(avail);
            } else {
              total = this._safeParseInt(size) * 1024;
              available = this._safeParseInt(avail) * 1024;
            }
          }
        }
      }

      if (total > 0 && available >= 0) {
        return { total, available };
      } else {
        console.warn(`Invalid values parsed: total=${total}, available=${available}`);
        return null;
      }
    } catch (error) {
      console.warn(`Command failed: ${command}`, error);
      return null;
    }
  }

  /**
   * Gets detailed mount information for debugging
   */
  private async _getMountInfo(path: string): Promise<{ filesystem: string; mountPoint: string; type: string } | null> {
    try {
      if (!fs.existsSync("/proc/mounts")) {
        return null;
      }

      const mountsContent = await fs.promises.readFile("/proc/mounts", "utf8");
      const lines = mountsContent.split("\n").filter((line) => line.trim());

      let bestMatch = null;
      let bestMatchLength = 0;

      for (const line of lines) {
        const parts = line.split(/\s+/);
        if (parts.length >= 3) {
          const [filesystem, mountPoint, type] = parts;

          if (path.startsWith(mountPoint) && mountPoint.length > bestMatchLength) {
            bestMatch = { filesystem, mountPoint, type };
            bestMatchLength = mountPoint.length;
          }
        }
      }

      return bestMatch;
    } catch (error) {
      console.warn(`Could not get mount info for ${path}:`, error);
      return null;
    }
  }

  /**
   * Detects if a path is a bind mount or mount point by checking /proc/mounts
   * Returns the actual filesystem path for bind mounts
   */
  private async _detectMountPoint(path: string): Promise<string | null> {
    try {
      if (!fs.existsSync("/proc/mounts")) {
        return null;
      }

      const mountsContent = await fs.promises.readFile("/proc/mounts", "utf8");
      const lines = mountsContent.split("\n").filter((line) => line.trim());

      let bestMatch = null;
      let bestMatchLength = 0;

      for (const line of lines) {
        const parts = line.split(/\s+/);
        if (parts.length >= 2) {
          const [, mountPoint] = parts;

          if (path.startsWith(mountPoint) && mountPoint.length > bestMatchLength) {
            bestMatch = mountPoint;
            bestMatchLength = mountPoint.length;
          }
        }
      }

      if (bestMatch && bestMatch !== "/") {
        return bestMatch;
      }

      return null;
    } catch (error) {
      console.warn(`Could not detect mount point for ${path}:`, error);
      return null;
    }
  }

  /**
   * Gets filesystem information for a specific path, with bind mount detection
   */
  private async _getFileSystemInfo(
    path: string
  ): Promise<{ total: number; available: number; mountPoint?: string } | null> {
    try {
      const mountInfo = await this._getMountInfo(path);
      if (mountInfo && mountInfo.mountPoint !== "/") {
        console.log(`📁 Bind mount detected: ${path} → ${mountInfo.filesystem} (${mountInfo.type})`);
      }

      const mountPoint = await this._detectMountPoint(path);
      const targetPath = mountPoint || path;

      const commandsToTry =
        process.platform === "win32"
          ? ["wmic logicaldisk get size,freespace,caption"]
          : process.platform === "darwin"
            ? [`df -k "${targetPath}"`, `df "${targetPath}"`]
            : [`df -B1 "${targetPath}"`, `df -k "${targetPath}"`, `df "${targetPath}"`];

      for (const command of commandsToTry) {
        const result = await this._tryDiskSpaceCommand(command);
        if (result) {
          return {
            ...result,
            mountPoint: mountPoint || undefined,
          };
        }
      }

      return null;
    } catch (error) {
      console.warn(`Error getting filesystem info for ${path}:`, error);
      return null;
    }
  }

  private async _getDiskSpaceMultiplePaths(): Promise<{ total: number; available: number } | null> {
    const pathsToTry = IS_RUNNING_IN_CONTAINER
      ? ["/app/server/uploads", "/app/server", "/app", "/"]
      : [".", "./uploads", process.cwd()];

    for (const pathToCheck of pathsToTry) {
      if (pathToCheck.includes("uploads")) {
        try {
          if (!fs.existsSync(pathToCheck)) {
            fs.mkdirSync(pathToCheck, { recursive: true });
          }
        } catch (err) {
          console.warn(`Could not create path ${pathToCheck}:`, err);
          continue;
        }
      }

      if (!fs.existsSync(pathToCheck)) {
        continue;
      }

      // Use the new filesystem detection method
      const result = await this._getFileSystemInfo(pathToCheck);
      if (result) {
        if (result.mountPoint) {
          console.log(`✅ Storage resolved via bind mount: ${result.mountPoint}`);
        }
        return { total: result.total, available: result.available };
      }
    }

    return null;
  }

  async getDiskSpace(
    userId?: string,
    isAdmin?: boolean
  ): Promise<{
    diskSizeGB: number;
    diskUsedGB: number;
    diskAvailableGB: number;
    uploadAllowed: boolean;
  }> {
    try {
      if (isAdmin) {
        const diskInfo = await this._getDiskSpaceMultiplePaths();

        if (!diskInfo) {
          console.error("❌ Could not determine disk space - system configuration issue");
          throw new Error("Unable to determine actual disk space - system configuration issue");
        }

        const { total, available } = diskInfo;
        const used = total - available;

        const diskSizeGB = this._ensureNumber(total / (1024 * 1024 * 1024), 0);
        const diskUsedGB = this._ensureNumber(used / (1024 * 1024 * 1024), 0);
        const diskAvailableGB = this._ensureNumber(available / (1024 * 1024 * 1024), 0);

        return {
          diskSizeGB: Number(diskSizeGB.toFixed(2)),
          diskUsedGB: Number(diskUsedGB.toFixed(2)),
          diskAvailableGB: Number(diskAvailableGB.toFixed(2)),
          uploadAllowed: diskAvailableGB > 0.1, // At least 100MB free
        };
      } else if (userId) {
        const maxTotalStorage = BigInt(await this.configService.getValue("maxTotalStoragePerUser"));
        const maxStorageGB = this._ensureNumber(Number(maxTotalStorage) / (1024 * 1024 * 1024), 10);

        const userFiles = await prisma.file.findMany({
          where: { userId },
          select: { size: true },
        });

        const totalUsedStorage = userFiles.reduce((acc, file) => acc + file.size, BigInt(0));

        const usedStorageGB = this._ensureNumber(Number(totalUsedStorage) / (1024 * 1024 * 1024), 0);
        const availableStorageGB = this._ensureNumber(maxStorageGB - usedStorageGB, 0);

        return {
          diskSizeGB: Number(maxStorageGB.toFixed(2)),
          diskUsedGB: Number(usedStorageGB.toFixed(2)),
          diskAvailableGB: Number(availableStorageGB.toFixed(2)),
          uploadAllowed: availableStorageGB > 0,
        };
      }

      throw new Error("User ID is required for non-admin users");
    } catch (error) {
      console.error("❌ Error getting disk space:", error);
      throw new Error(
        `Failed to get disk space information: ${error instanceof Error ? error.message : String(error)}`
      );
    }
  }

  async checkUploadAllowed(
    fileSize: number,
    userId?: string
  ): Promise<{
    diskSizeGB: number;
    diskUsedGB: number;
    diskAvailableGB: number;
    uploadAllowed: boolean;
    fileSizeInfo: {
      bytes: number;
      kb: number;
      mb: number;
      gb: number;
    };
  }> {
    const diskSpace = await this.getDiskSpace(userId);
    const fileSizeGB = fileSize / (1024 * 1024 * 1024);

    return {
      ...diskSpace,
      uploadAllowed: diskSpace.diskAvailableGB > fileSizeGB,
      fileSizeInfo: {
        bytes: fileSize,
        kb: Number((fileSize / 1024).toFixed(2)),
        mb: Number((fileSize / (1024 * 1024)).toFixed(2)),
        gb: Number((fileSize / (1024 * 1024 * 1024)).toFixed(2)),
      },
    };
  }
}
