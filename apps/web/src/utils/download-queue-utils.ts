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

export async function bulkDownloadWithQueue(
  files: Array<{
    objectName?: string;
    name: string;
    id?: string;
    isReverseShare?: boolean;
  }>,
  zipName: string,
  onProgress?: (current: number, total: number) => void
): Promise<void> {
  try {
    const JSZip = (await import("jszip")).default;
    const zip = new JSZip();

    const downloadPromises = files.map(async (file, index) => {
      try {
        const blob = await downloadFileAsBlobWithQueue(
          file.objectName || file.name,
          file.name,
          file.isReverseShare,
          file.id
        );
        zip.file(file.name, blob);
        onProgress?.(index + 1, files.length);
      } catch (error) {
        console.error(`Error downloading file ${file.name}:`, error);
        throw error;
      }
    });

    await Promise.all(downloadPromises);

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
