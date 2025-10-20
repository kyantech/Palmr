import { prisma } from "../shared/prisma";

/**
 * Generates a unique filename by checking for duplicates in the database
 * and appending a numeric suffix if necessary (e.g., file (1).txt, file (2).txt)
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
  let uniqueName = `${baseName} (${suffix}).${extension}`;

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
    uniqueName = `${baseName} (${suffix}).${extension}`;
  }
}

/**
 * Generates a unique filename for rename operations by checking for duplicates
 * and appending a numeric suffix if necessary (e.g., file (1).txt, file (2).txt)
 *
 * @param baseName - The original filename without extension
 * @param extension - The file extension
 * @param userId - The user ID who owns the file
 * @param folderId - The folder ID where the file will be stored (null for root)
 * @param excludeFileId - The ID of the file being renamed (to exclude from duplicate check)
 * @returns A unique filename with extension
 */
export async function generateUniqueFileNameForRename(
  baseName: string,
  extension: string,
  userId: string,
  folderId: string | null | undefined,
  excludeFileId: string
): Promise<string> {
  const fullName = `${baseName}.${extension}`;
  const targetFolderId = folderId || null;

  // Check if the original filename exists in the target folder (excluding current file)
  const existingFile = await prisma.file.findFirst({
    where: {
      name: fullName,
      userId,
      folderId: targetFolderId,
      id: { not: excludeFileId },
    },
  });

  // If no duplicate, return the original name
  if (!existingFile) {
    return fullName;
  }

  // Find the next available suffix number
  let suffix = 1;
  let uniqueName = `${baseName} (${suffix}).${extension}`;

  while (true) {
    const duplicateFile = await prisma.file.findFirst({
      where: {
        name: uniqueName,
        userId,
        folderId: targetFolderId,
        id: { not: excludeFileId },
      },
    });

    if (!duplicateFile) {
      return uniqueName;
    }

    suffix++;
    uniqueName = `${baseName} (${suffix}).${extension}`;
  }
}

/**
 * Generates a unique folder name by checking for duplicates in the database
 * and appending a numeric suffix if necessary (e.g., folder (1), folder (2))
 *
 * @param name - The original folder name
 * @param userId - The user ID who owns the folder
 * @param parentId - The parent folder ID (null for root)
 * @param excludeFolderId - The ID of the folder being renamed (to exclude from duplicate check)
 * @returns A unique folder name
 */
export async function generateUniqueFolderName(
  name: string,
  userId: string,
  parentId: string | null | undefined,
  excludeFolderId?: string
): Promise<string> {
  const targetParentId = parentId || null;

  // Build the where clause
  const whereClause: any = {
    name,
    userId,
    parentId: targetParentId,
  };

  // Exclude the current folder if this is a rename operation
  if (excludeFolderId) {
    whereClause.id = { not: excludeFolderId };
  }

  // Check if the original folder name exists in the target location
  const existingFolder = await prisma.folder.findFirst({
    where: whereClause,
  });

  // If no duplicate, return the original name
  if (!existingFolder) {
    return name;
  }

  // Find the next available suffix number
  let suffix = 1;
  let uniqueName = `${name} (${suffix})`;

  while (true) {
    const whereClauseForSuffix: any = {
      name: uniqueName,
      userId,
      parentId: targetParentId,
    };

    if (excludeFolderId) {
      whereClauseForSuffix.id = { not: excludeFolderId };
    }

    const duplicateFolder = await prisma.folder.findFirst({
      where: whereClauseForSuffix,
    });

    if (!duplicateFolder) {
      return uniqueName;
    }

    suffix++;
    uniqueName = `${name} (${suffix})`;
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
