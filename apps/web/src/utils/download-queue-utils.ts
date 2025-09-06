import { toast } from "sonner";

import { getDownloadUrl } from "@/http/endpoints";
import { downloadReverseShareFile } from "@/http/endpoints/reverse-shares";

interface DownloadWithQueueOptions {
  useQueue?: boolean;
  silent?: boolean;
  showToasts?: boolean;
  onStart?: (downloadId: string) => void;
  onComplete?: (downloadId: string) => void;
  onFail?: (downloadId: string, error: string) => void;
}

async function waitForDownloadReady(objectName: string, fileName: string): Promise<string> {
  let attempts = 0;
  const maxAttempts = 30;
  let currentDelay = 2000;
  const maxDelay = 10000;

  while (attempts < maxAttempts) {
    try {
      const encodedObjectName = encodeURIComponent(objectName);
      const response = await getDownloadUrl(encodedObjectName);

      if (response.status !== 202) {
        return response.data.url;
      }

      await new Promise((resolve) => setTimeout(resolve, currentDelay));

      if (attempts > 3 && currentDelay < maxDelay) {
        currentDelay = Math.min(currentDelay * 1.5, maxDelay);
      }

      attempts++;
    } catch (error) {
      console.error(`Error checking download status for ${fileName}:`, error);
      await new Promise((resolve) => setTimeout(resolve, currentDelay * 2));
      attempts++;
    }
  }

  throw new Error(`Download timeout for ${fileName} after ${attempts} attempts`);
}

async function waitForReverseShareDownloadReady(fileId: string, fileName: string): Promise<string> {
  let attempts = 0;
  const maxAttempts = 30;
  let currentDelay = 2000;
  const maxDelay = 10000;

  while (attempts < maxAttempts) {
    try {
      const response = await downloadReverseShareFile(fileId);

      if (response.status !== 202) {
        return response.data.url;
      }

      await new Promise((resolve) => setTimeout(resolve, currentDelay));

      if (attempts > 3 && currentDelay < maxDelay) {
        currentDelay = Math.min(currentDelay * 1.5, maxDelay);
      }

      attempts++;
    } catch (error) {
      console.error(`Error checking reverse share download status for ${fileName}:`, error);
      await new Promise((resolve) => setTimeout(resolve, currentDelay * 2));
      attempts++;
    }
  }

  throw new Error(`Reverse share download timeout for ${fileName} after ${attempts} attempts`);
}

async function performDownload(url: string, fileName: string): Promise<void> {
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
}

export async function downloadFileWithQueue(
  objectName: string,
  fileName: string,
  options: DownloadWithQueueOptions = {}
): Promise<void> {
  const { useQueue = true, silent = false, showToasts = true } = options;
  const downloadId = `${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;

  try {
    if (!silent) {
      options.onStart?.(downloadId);
    }

    const encodedObjectName = encodeURIComponent(objectName);
    const response = await getDownloadUrl(encodedObjectName);

    if (response.status === 202 && useQueue) {
      if (!silent && showToasts) {
        toast.info(`${fileName} was added to download queue`, {
          description: "Download will start automatically when queue space is available",
          duration: 5000,
        });
      }

      const actualDownloadUrl = await waitForDownloadReady(objectName, fileName);
      await performDownload(actualDownloadUrl, fileName);
    } else {
      await performDownload(response.data.url, fileName);
    }

    if (!silent) {
      options.onComplete?.(downloadId);
      if (showToasts) {
        toast.success(`${fileName} downloaded successfully`);
      }
    }
  } catch (error: any) {
    if (!silent) {
      options.onFail?.(downloadId, error?.message || "Download failed");
      if (showToasts) {
        toast.error(`Failed to download ${fileName}`);
      }
    }
    throw error;
  }
}

export async function downloadReverseShareWithQueue(
  fileId: string,
  fileName: string,
  options: DownloadWithQueueOptions = {}
): Promise<void> {
  const { silent = false, showToasts = true } = options;
  const downloadId = `reverse-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;

  try {
    if (!silent) {
      options.onStart?.(downloadId);
    }

    const response = await downloadReverseShareFile(fileId);

    if (response.status === 202) {
      if (!silent && showToasts) {
        toast.info(`${fileName} was added to download queue`, {
          description: "Download will start automatically when queue space is available",
          duration: 5000,
        });
      }

      const actualDownloadUrl = await waitForReverseShareDownloadReady(fileId, fileName);
      await performDownload(actualDownloadUrl, fileName);
    } else {
      await performDownload(response.data.url, fileName);
    }

    if (!silent) {
      options.onComplete?.(downloadId);
      if (showToasts) {
        toast.success(`${fileName} downloaded successfully`);
      }
    }
  } catch (error: any) {
    if (!silent) {
      options.onFail?.(downloadId, error?.message || "Download failed");
      if (showToasts) {
        toast.error(`Failed to download ${fileName}`);
      }
    }
    throw error;
  }
}

export async function downloadFileAsBlobWithQueue(
  objectName: string,
  fileName: string,
  isReverseShare: boolean = false,
  fileId?: string
): Promise<Blob> {
  try {
    let downloadUrl: string;

    if (isReverseShare && fileId) {
      const response = await downloadReverseShareFile(fileId);

      if (response.status === 202) {
        downloadUrl = await waitForReverseShareDownloadReady(fileId, fileName);
      } else {
        downloadUrl = response.data.url;
      }
    } else {
      const encodedObjectName = encodeURIComponent(objectName);
      const response = await getDownloadUrl(encodedObjectName);

      if (response.status === 202) {
        downloadUrl = await waitForDownloadReady(objectName, fileName);
      } else {
        downloadUrl = response.data.url;
      }
    }

    const fetchResponse = await fetch(downloadUrl);
    if (!fetchResponse.ok) {
      throw new Error(`Failed to download ${fileName}: ${fetchResponse.status}`);
    }

    return await fetchResponse.blob();
  } catch (error: any) {
    console.error(`Error downloading ${fileName}:`, error);
    throw error;
  }
}

// Helper function to collect all files in a folder recursively
function collectFolderFiles(
  folderId: string,
  allFiles: any[],
  allFolders: any[],
  folderPath: string = ""
): Array<{ objectName: string; name: string; zipPath: string }> {
  const result: Array<{ objectName: string; name: string; zipPath: string }> = [];

  // Get direct files in this folder
  const directFiles = allFiles.filter((file: any) => file.folderId === folderId);
  for (const file of directFiles) {
    result.push({
      objectName: file.objectName,
      name: file.name,
      zipPath: folderPath + file.name,
    });
  }

  // Get subfolders and collect their files recursively
  const subfolders = allFolders.filter((folder: any) => folder.parentId === folderId);
  for (const subfolder of subfolders) {
    const subfolderPath = folderPath + subfolder.name + "/";
    const subFiles = collectFolderFiles(subfolder.id, allFiles, allFolders, subfolderPath);
    result.push(...subFiles);
  }

  return result;
}

// Helper function to collect empty folders
function collectEmptyFolders(folderId: string, allFiles: any[], allFolders: any[], folderPath: string = ""): string[] {
  const emptyFolders: string[] = [];

  // Get subfolders
  const subfolders = allFolders.filter((folder: any) => folder.parentId === folderId);
  for (const subfolder of subfolders) {
    const subfolderPath = folderPath + subfolder.name + "/";

    // Check if this subfolder has any files (recursively)
    const subfolderFiles = collectFolderFiles(subfolder.id, allFiles, allFolders, "");

    if (subfolderFiles.length === 0) {
      // This folder is empty, add it
      emptyFolders.push(subfolderPath.slice(0, -1)); // Remove trailing slash
    }

    // Recursively check for empty subfolders
    const nestedEmptyFolders = collectEmptyFolders(subfolder.id, allFiles, allFolders, subfolderPath);
    emptyFolders.push(...nestedEmptyFolders);
  }

  return emptyFolders;
}

// Download a single folder from file manager
export async function downloadFolderWithQueue(
  folderId: string,
  folderName: string,
  options: DownloadWithQueueOptions = {}
): Promise<void> {
  const { silent = false, showToasts = true } = options;
  const downloadId = `folder-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;

  try {
    if (!silent) {
      options.onStart?.(downloadId);
    }

    // Get all files and folders from the system
    const { listFiles } = await import("@/http/endpoints/files");
    const { listFolders } = await import("@/http/endpoints/folders");

    const [allFilesResponse, allFoldersResponse] = await Promise.all([listFiles(), listFolders()]);
    const allFiles = allFilesResponse.data.files || [];
    const allFolders = allFoldersResponse.data.folders || [];

    // Collect all files in this folder recursively, wrapped in folder name
    const folderFiles = collectFolderFiles(folderId, allFiles, allFolders, `${folderName}/`);
    const emptyFolders = collectEmptyFolders(folderId, allFiles, allFolders, `${folderName}/`);

    if (folderFiles.length === 0 && emptyFolders.length === 0) {
      const message = "Folder is empty";
      if (showToasts) {
        toast.error(message);
      }
      throw new Error(message);
    }

    // Create ZIP following original pattern
    const JSZip = (await import("jszip")).default;
    const zip = new JSZip();

    // Add empty folders first
    for (const emptyFolderPath of emptyFolders) {
      zip.folder(emptyFolderPath);
    }

    // Download and add files to ZIP
    for (const file of folderFiles) {
      try {
        const blob = await downloadFileAsBlobWithQueue(file.objectName, file.name);
        zip.file(file.zipPath, blob);
      } catch (error) {
        console.error(`Error downloading file ${file.name}:`, error);
        // Continue with other files
      }
    }

    // Generate and download ZIP
    const zipBlob = await zip.generateAsync({ type: "blob" });
    const url = URL.createObjectURL(zipBlob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${folderName}.zip`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);

    if (!silent) {
      options.onComplete?.(downloadId);
      if (showToasts) {
        toast.success(`${folderName} downloaded successfully`);
      }
    }
  } catch (error: any) {
    if (!silent) {
      options.onFail?.(downloadId, error?.message || "Download failed");
      if (showToasts) {
        toast.error(`Failed to download ${folderName}`);
      }
    }
    throw error;
  }
}

// Download a single folder from share data
export async function downloadShareFolderWithQueue(
  folderId: string,
  folderName: string,
  shareFiles: any[],
  shareFolders: any[],
  options: DownloadWithQueueOptions = {}
): Promise<void> {
  const { silent = false, showToasts = true } = options;
  const downloadId = `share-folder-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;

  try {
    if (!silent) {
      options.onStart?.(downloadId);
    }

    // Collect all files in this folder recursively using share data, wrapped in folder name
    const folderFiles = collectFolderFiles(folderId, shareFiles, shareFolders, `${folderName}/`);
    const emptyFolders = collectEmptyFolders(folderId, shareFiles, shareFolders, `${folderName}/`);

    if (folderFiles.length === 0 && emptyFolders.length === 0) {
      const message = "Folder is empty";
      if (showToasts) {
        toast.error(message);
      }
      throw new Error(message);
    }

    // Create ZIP following original pattern
    const JSZip = (await import("jszip")).default;
    const zip = new JSZip();

    // Add empty folders first
    for (const emptyFolderPath of emptyFolders) {
      zip.folder(emptyFolderPath);
    }

    // Download and add files to ZIP
    for (const file of folderFiles) {
      try {
        const blob = await downloadFileAsBlobWithQueue(file.objectName, file.name);
        zip.file(file.zipPath, blob);
      } catch (error) {
        console.error(`Error downloading file ${file.name}:`, error);
        // Continue with other files
      }
    }

    // Generate and download ZIP
    const zipBlob = await zip.generateAsync({ type: "blob" });
    const url = URL.createObjectURL(zipBlob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${folderName}.zip`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);

    if (!silent) {
      options.onComplete?.(downloadId);
      if (showToasts) {
        toast.success(`${folderName} downloaded successfully`);
      }
    }
  } catch (error: any) {
    if (!silent) {
      options.onFail?.(downloadId, error?.message || "Download failed");
      if (showToasts) {
        toast.error(`Failed to download ${folderName}`);
      }
    }
    throw error;
  }
}

// Bulk download from file manager - extends original pattern
export async function bulkDownloadWithQueue(
  items: Array<{
    objectName?: string;
    name: string;
    id?: string;
    isReverseShare?: boolean;
    type?: "file" | "folder";
  }>,
  zipName: string,
  onProgress?: (current: number, total: number) => void,
  wrapInFolder?: boolean
): Promise<void> {
  try {
    const JSZip = (await import("jszip")).default;
    const zip = new JSZip();

    const files = items.filter((item) => item.type !== "folder");
    const folders = items.filter((item) => item.type === "folder");

    // eslint-disable-next-line prefer-const
    let allFilesToDownload: Array<{ objectName: string; name: string; zipPath: string }> = [];
    // eslint-disable-next-line prefer-const
    let allEmptyFolders: string[] = [];

    // If we have folders, get system data to resolve them
    if (folders.length > 0) {
      const { listFiles } = await import("@/http/endpoints/files");
      const { listFolders } = await import("@/http/endpoints/folders");

      const [allFilesResponse, allFoldersResponse] = await Promise.all([listFiles(), listFolders()]);
      const allFiles = allFilesResponse.data.files || [];
      const allFolders = allFoldersResponse.data.folders || [];

      // Determine wrapper path
      const wrapperPath = wrapInFolder ? `${zipName.replace(".zip", "")}/` : "";

      // Collect ALL files from ALL folders (no filtering by selected files)
      for (const folder of folders) {
        const folderPath = wrapperPath + `${folder.name}/`;
        const folderFiles = collectFolderFiles(folder.id!, allFiles, allFolders, folderPath);
        const emptyFolders = collectEmptyFolders(folder.id!, allFiles, allFolders, folderPath);

        allFilesToDownload.push(...folderFiles);
        allEmptyFolders.push(...emptyFolders);
      }

      // Get set of files that are already included in folders
      const filesInFolders = new Set(allFilesToDownload.map((f) => f.objectName));

      // Add individual files that aren't already included in folders
      for (const file of files) {
        if (!file.objectName || !filesInFolders.has(file.objectName)) {
          allFilesToDownload.push({
            objectName: file.objectName || file.name,
            name: file.name,
            zipPath: wrapperPath + file.name,
          });
        }
      }
    } else {
      // No folders, just process files like original
      const wrapperPath = wrapInFolder ? `${zipName.replace(".zip", "")}/` : "";
      for (const file of files) {
        allFilesToDownload.push({
          objectName: file.objectName || file.name,
          name: file.name,
          zipPath: wrapperPath + file.name,
        });
      }
    }

    // Add empty folders to ZIP
    for (const emptyFolderPath of allEmptyFolders) {
      zip.folder(emptyFolderPath);
    }

    // Download and add all files to ZIP
    for (let i = 0; i < allFilesToDownload.length; i++) {
      const file = allFilesToDownload[i];
      try {
        const blob = await downloadFileAsBlobWithQueue(file.objectName, file.name);
        zip.file(file.zipPath, blob);
        onProgress?.(i + 1, allFilesToDownload.length);
      } catch (error) {
        console.error(`Error downloading file ${file.name}:`, error);
        // Continue with other files
      }
    }

    // Generate and download ZIP like original
    const zipBlob = await zip.generateAsync({ type: "blob" });
    const url = URL.createObjectURL(zipBlob);
    const a = document.createElement("a");
    a.href = url;
    a.download = zipName.endsWith(".zip") ? zipName : `${zipName}.zip`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  } catch (error) {
    console.error("Error creating ZIP:", error);
    throw error;
  }
}

// Bulk download from share data - clean version for shares
export async function bulkDownloadShareWithQueue(
  items: Array<{
    objectName?: string;
    name: string;
    id?: string;
    type?: "file" | "folder";
  }>,
  shareFiles: any[],
  shareFolders: any[],
  zipName: string,
  onProgress?: (current: number, total: number) => void,
  wrapInFolder?: boolean
): Promise<void> {
  try {
    const JSZip = (await import("jszip")).default;
    const zip = new JSZip();

    const files = items.filter((item) => item.type !== "folder");
    const folders = items.filter((item) => item.type === "folder");

    // eslint-disable-next-line prefer-const
    let allFilesToDownload: Array<{ objectName: string; name: string; zipPath: string }> = [];
    // eslint-disable-next-line prefer-const
    let allEmptyFolders: string[] = [];

    // Determine wrapper path
    const wrapperPath = wrapInFolder ? `${zipName.replace(".zip", "")}/` : "";

    // Collect ALL files from ALL folders using share data (no filtering)
    for (const folder of folders) {
      const folderPath = wrapperPath + `${folder.name}/`;
      const folderFiles = collectFolderFiles(folder.id!, shareFiles, shareFolders, folderPath);
      const emptyFolders = collectEmptyFolders(folder.id!, shareFiles, shareFolders, folderPath);

      allFilesToDownload.push(...folderFiles);
      allEmptyFolders.push(...emptyFolders);
    }

    // Get set of files that are already included in folders
    const filesInFolders = new Set(allFilesToDownload.map((f) => f.objectName));

    // Add individual files that aren't already included in folders
    for (const file of files) {
      if (!file.objectName || !filesInFolders.has(file.objectName)) {
        allFilesToDownload.push({
          objectName: file.objectName!,
          name: file.name,
          zipPath: wrapperPath + file.name,
        });
      }
    }

    // Add empty folders to ZIP
    for (const emptyFolderPath of allEmptyFolders) {
      zip.folder(emptyFolderPath);
    }

    // Download and add all files to ZIP
    for (let i = 0; i < allFilesToDownload.length; i++) {
      const file = allFilesToDownload[i];
      try {
        const blob = await downloadFileAsBlobWithQueue(file.objectName, file.name);
        zip.file(file.zipPath, blob);
        onProgress?.(i + 1, allFilesToDownload.length);
      } catch (error) {
        console.error(`Error downloading file ${file.name}:`, error);
        // Continue with other files
      }
    }

    // Generate and download ZIP like original
    const zipBlob = await zip.generateAsync({ type: "blob" });
    const url = URL.createObjectURL(zipBlob);
    const a = document.createElement("a");
    a.href = url;
    a.download = zipName.endsWith(".zip") ? zipName : `${zipName}.zip`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  } catch (error) {
    console.error("Error creating ZIP:", error);
    throw error;
  }
}
