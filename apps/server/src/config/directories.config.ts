import * as path from "path";

import { IS_RUNNING_IN_CONTAINER } from "../utils/container-detection";

/**
 * Directory Configuration for Palmr Server
 *
 * This configuration manages all directory paths used by the server,
 * including temporary directories for uploads.
 */

export interface DirectoryConfig {
  baseDir: string;
  uploads: string;
  tempUploads: string;
}

const BASE_DIR = IS_RUNNING_IN_CONTAINER ? "/app/server" : process.cwd();

export const directoriesConfig: DirectoryConfig = {
  baseDir: BASE_DIR,
  uploads: path.join(BASE_DIR, "uploads"),
  tempUploads: path.join(BASE_DIR, "temp-uploads"),
};

/**
 * Get the temporary directory for upload operations
 * This is where files are temporarily stored during streaming uploads
 */
export function getTempUploadDir(): string {
  return directoriesConfig.tempUploads;
}

/**
 * Get the uploads directory
 * This is where final files are stored
 */
export function getUploadsDir(): string {
  return directoriesConfig.uploads;
}

/**
 * Get temporary path for a file during upload
 * This ensures unique temporary file names to avoid conflicts
 * Files are stored directly in temp-uploads with timestamp + random suffix
 */
export function getTempFilePath(objectName: string): string {
  const sanitizedName = objectName.replace(/[^a-zA-Z0-9\-_./]/g, "_");
  const timestamp = Date.now();
  const randomSuffix = Math.random().toString(36).substring(2, 8);
  return path.join(getTempUploadDir(), `${timestamp}-${randomSuffix}-${sanitizedName}.tmp`);
}
