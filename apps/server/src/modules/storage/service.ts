import { exec } from "child_process";
import fs from "node:fs";
import { promisify } from "util";
import { PrismaClient } from "@prisma/client";

import { env } from "../../env";
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

  private _parseSize(value: string): number {
    if (!value) return 0;

    const cleanValue = value.trim().toLowerCase();

    const numericMatch = cleanValue.match(/^(\d+(?:\.\d+)?)/);
    if (!numericMatch) return 0;

    const numericValue = parseFloat(numericMatch[1]);
    if (Number.isNaN(numericValue)) return 0;

    if (cleanValue.includes("t")) {
      return Math.round(numericValue * 1024 * 1024 * 1024 * 1024);
    } else if (cleanValue.includes("g")) {
      return Math.round(numericValue * 1024 * 1024 * 1024);
    } else if (cleanValue.includes("m")) {
      return Math.round(numericValue * 1024 * 1024);
    } else if (cleanValue.includes("k")) {
      return Math.round(numericValue * 1024);
    } else {
      return Math.round(numericValue);
    }
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

        if (command.includes("findmnt")) {
          if (lines.length >= 1) {
            const parts = lines[0].trim().split(/\s+/);
            if (parts.length >= 2) {
              const [availStr, sizeStr] = parts;
              available = this._parseSize(availStr);
              total = this._parseSize(sizeStr);
            }
          }
        } else if (command.includes("stat -f")) {
          let blockSize = 0;
          let totalBlocks = 0;
          let freeBlocks = 0;

          for (const line of lines) {
            if (line.includes("Block size:")) {
              blockSize = this._safeParseInt(line.split(":")[1].trim());
            } else if (line.includes("Total blocks:")) {
              totalBlocks = this._safeParseInt(line.split(":")[1].trim());
            } else if (line.includes("Free blocks:")) {
              freeBlocks = this._safeParseInt(line.split(":")[1].trim());
            }
          }

          if (blockSize > 0 && totalBlocks > 0) {
            total = totalBlocks * blockSize;
            available = freeBlocks * blockSize;
          } else {
            return null;
          }
        } else if (command.includes("--output=")) {
          if (lines.length >= 2) {
            const parts = lines[1].trim().split(/\s+/);
            if (parts.length >= 2) {
              const [availStr, sizeStr] = parts;
              available = this._safeParseInt(availStr) * 1024;
              total = this._safeParseInt(sizeStr) * 1024;
            }
          }
        } else {
          if (lines.length >= 2) {
            const parts = lines[1].trim().split(/\s+/);
            if (parts.length >= 4) {
              const [, size, , avail] = parts;
              if (command.includes("-B1")) {
                total = this._safeParseInt(size);
                available = this._safeParseInt(avail);
              } else if (command.includes("-h")) {
                total = this._parseSize(size);
                available = this._parseSize(avail);
              } else {
                total = this._safeParseInt(size) * 1024;
                available = this._safeParseInt(avail) * 1024;
              }
            }
          }
        }
      }

      if (total > 0 && available >= 0) {
        return { total, available };
      } else {
        return null;
      }
    } catch {
      return null;
    }
  }

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
    } catch {
      return null;
    }
  }

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
        if (parts.length >= 3) {
          const [device, mountPoint, filesystem] = parts;
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
    } catch {
      return null;
    }
  }

  private async _getFileSystemInfo(
    path: string
  ): Promise<{ total: number; available: number; mountPoint?: string } | null> {
    try {
      const mountInfo = await this._getMountInfo(path);
      const mountPoint = await this._detectMountPoint(path);
      const targetPath = mountPoint || path;

      const commandsToTry =
        process.platform === "win32"
          ? ["wmic logicaldisk get size,freespace,caption"]
          : process.platform === "darwin"
            ? [`df -k "${targetPath}"`, `df "${targetPath}"`]
            : [
                `df -B1 "${targetPath}"`,
                `df -k "${targetPath}"`,
                `df "${targetPath}"`,
                `df -h "${targetPath}"`,
                `df -T "${targetPath}"`,
                `stat -f "${targetPath}"`,
                `findmnt -n -o AVAIL,SIZE "${targetPath}"`,
                `findmnt -n -o AVAIL,SIZE,TARGET "${targetPath}"`,
                `df -P "${targetPath}"`,
                `df --output=avail,size "${targetPath}"`,
              ];

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
    } catch {
      return null;
    }
  }

  private async _detectSynologyVolumes(): Promise<string[]> {
    try {
      if (!fs.existsSync("/proc/mounts")) {
        return [];
      }

      const mountsContent = await fs.promises.readFile("/proc/mounts", "utf8");
      const lines = mountsContent.split("\n").filter((line) => line.trim());
      const synologyPaths: string[] = [];

      for (const line of lines) {
        const parts = line.split(/\s+/);
        if (parts.length >= 2) {
          const [, mountPoint] = parts;

          if (mountPoint.match(/^\/volume\d+$/)) {
            synologyPaths.push(mountPoint);
          }
        }
      }

      return synologyPaths;
    } catch {
      return [];
    }
  }

  private async _getDiskSpaceMultiplePaths(): Promise<{ total: number; available: number } | null> {
    const basePaths = IS_RUNNING_IN_CONTAINER
      ? ["/app/server/uploads", "/app/server/temp-uploads", "/app/server/temp-chunks", "/app/server", "/app", "/"]
      : [".", "./uploads", process.cwd()];

    const synologyPaths = await this._detectSynologyVolumes();

    const pathsToTry = [...basePaths, ...synologyPaths];

    for (const pathToCheck of pathsToTry) {
      if (pathToCheck.includes("uploads") || pathToCheck.includes("temp-")) {
        try {
          if (!fs.existsSync(pathToCheck)) {
            fs.mkdirSync(pathToCheck, { recursive: true });
          }
        } catch {
          continue;
        }
      }

      if (!fs.existsSync(pathToCheck)) {
        continue;
      }

      const result = await this._getFileSystemInfo(pathToCheck);
      if (result) {
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
          uploadAllowed: diskAvailableGB > 0.1,
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
      console.error("Error getting disk space:", error);
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
