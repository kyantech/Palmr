import JSZip from "jszip";

interface DownloadItem {
  url: string;
  name: string;
}

/**
 * Downloads multiple files and creates a ZIP archive in the browser
 * @param items Array of items with presigned URLs and file names
 * @param zipName Name for the ZIP file
 */
export async function downloadFilesAsZip(items: DownloadItem[], zipName: string): Promise<void> {
  const zip = new JSZip();

  // Download all files and add to ZIP
  const promises = items.map(async (item) => {
    try {
      const response = await fetch(item.url);
      if (!response.ok) {
        throw new Error(`Failed to download ${item.name}`);
      }
      const blob = await response.blob();
      zip.file(item.name, blob);
    } catch (error) {
      console.error(`Error downloading ${item.name}:`, error);
      throw error;
    }
  });

  await Promise.all(promises);

  // Generate ZIP file
  const zipBlob = await zip.generateAsync({ type: "blob" });

  // Trigger download
  const url = URL.createObjectURL(zipBlob);
  const link = document.createElement("a");
  link.href = url;
  link.download = zipName;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  URL.revokeObjectURL(url);
}

/**
 * Downloads a single file directly
 * @param url Presigned URL
 * @param fileName File name
 */
export async function downloadFile(url: string, fileName: string): Promise<void> {
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
}
