import { prisma } from "../shared/prisma";

/**
 * Generates a unique filename by checking for duplicates in the database
 * and appending a numeric suffix if necessary (e.g., file(1).txt, file(2).txt)
 *
 * @param baseName - The original filename without extension
 * @param extension - The file extension
 * @param userId - The user ID who owns the file
 * @param folderId - The folder ID where the file will be stored (null for root)
 * @returns A unique filename with extension
 */
export async function generateUniqueFileName(
  baseName: string,
  extension: string,
  userId: string,
  folderId: string | null | undefined
): Promise<string> {
  const fullName = `${baseName}.${extension}`;
  const targetFolderId = folderId || null;

  // Check if the original filename exists in the target folder
  const existingFile = await prisma.file.findFirst({
    where: {
      name: fullName,
      userId,
      folderId: targetFolderId,
    },
  });

  // If no duplicate, return the original name
  if (!existingFile) {
    return fullName;
  }

  // Find the next available suffix number
  let suffix = 1;
  let uniqueName = `${baseName}(${suffix}).${extension}`;

  while (true) {
    const duplicateFile = await prisma.file.findFirst({
      where: {
        name: uniqueName,
        userId,
        folderId: targetFolderId,
      },
    });

    if (!duplicateFile) {
      return uniqueName;
    }

    suffix++;
    uniqueName = `${baseName}(${suffix}).${extension}`;
  }
}

/**
 * Parses a filename into base name and extension
 *
 * @param filename - The full filename with extension
 * @returns Object with baseName and extension
 */
export function parseFileName(filename: string): { baseName: string; extension: string } {
  const lastDotIndex = filename.lastIndexOf(".");

  if (lastDotIndex === -1 || lastDotIndex === 0) {
    // No extension or hidden file with no name before dot
    return {
      baseName: filename,
      extension: "",
    };
  }

  return {
    baseName: filename.substring(0, lastDotIndex),
    extension: filename.substring(lastDotIndex + 1),
  };
}
