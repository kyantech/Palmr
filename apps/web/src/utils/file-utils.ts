import { nanoid } from "nanoid";

export function generateSafeFileName(originalName: string): string {
  const extension = originalName.split(".").pop() || "";
  const safeId = nanoid().replace(/[-_]/g, "").slice(0, 12);

  return `${safeId}.${extension}`;
}

/**
 * Intelligently truncates a filename while preserving the extension when possible
 * @param fileName - Filename to truncate
 * @param maxLength - Maximum length of the name (default: 40)
 * @returns Truncated filename
 */
export function truncateFileName(fileName: string, maxLength: number = 40): string {
  if (fileName.length <= maxLength) return fileName;

  const lastDotIndex = fileName.lastIndexOf(".");

  if (lastDotIndex > 0 && lastDotIndex > fileName.length - 10) {
    const name = fileName.substring(0, lastDotIndex);
    const extension = fileName.substring(lastDotIndex);
    const availableLength = maxLength - extension.length - 3;

    if (availableLength > 0) {
      return `${name.substring(0, availableLength)}...${extension}`;
    }
  }

  const halfLength = Math.floor((maxLength - 3) / 2);
  return `${fileName.substring(0, halfLength)}...${fileName.substring(fileName.length - halfLength)}`;
}

interface FileItem {
  name: string;
  folderId?: string | null;
}

/**
 * Generates a unique filename by adding a suffix (1), (2), etc. if a file with the same name already exists
 * @param fileName - Original filename to check
 * @param existingFiles - Array of existing files in the same folder
 * @param currentFolderId - Current folder ID (null for root)
 * @returns Unique filename with suffix if needed
 */
export function generateUniqueFileName(
  fileName: string,
  existingFiles: FileItem[],
  currentFolderId?: string | null
): string {
  // Filter files in the same folder
  const filesInFolder = existingFiles.filter((file) => (file.folderId || null) === (currentFolderId || null));

  // Extract name and extension
  const lastDotIndex = fileName.lastIndexOf(".");
  const baseName = lastDotIndex > 0 ? fileName.substring(0, lastDotIndex) : fileName;
  const extension = lastDotIndex > 0 ? fileName.substring(lastDotIndex) : "";

  // Check if the filename already exists
  const fileExists = filesInFolder.some((file) => file.name === fileName);

  if (!fileExists) {
    return fileName;
  }

  // Find the next available number
  let counter = 1;
  let newFileName = `${baseName} (${counter})${extension}`;

  while (filesInFolder.some((file) => file.name === newFileName)) {
    counter++;
    newFileName = `${baseName} (${counter})${extension}`;
  }

  return newFileName;
}
