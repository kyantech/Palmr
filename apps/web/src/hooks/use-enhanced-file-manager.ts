import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { deleteFile, getDownloadUrl, updateFile } from "@/http/endpoints";
import { useDownloadQueue } from "./use-download-queue";
import { usePushNotifications } from "./use-push-notifications";

interface FileToRename {
  id: string;
  name: string;
  description?: string;
}

interface FileToDelete {
  id: string;
  name: string;
}

interface PreviewFile {
  name: string;
  objectName: string;
  description?: string;
}

interface FileToShare {
  id: string;
  name: string;
  description?: string;
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface BulkFile {
  id: string;
  name: string;
  description?: string;
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface PendingDownload {
  downloadId: string;
  fileName: string;
  objectName: string;
  startTime: number;
  status: "pending" | "queued" | "downloading" | "completed" | "failed";
}

export interface EnhancedFileManagerHook {
  previewFile: PreviewFile | null;
  fileToDelete: any;
  fileToRename: any;
  fileToShare: FileToShare | null;
  filesToDelete: BulkFile[] | null;
  filesToShare: BulkFile[] | null;
  filesToDownload: BulkFile[] | null;
  isBulkDownloadModalOpen: boolean;
  pendingDownloads: PendingDownload[];
  setFileToDelete: (file: any) => void;
  setFileToRename: (file: any) => void;
  setPreviewFile: (file: PreviewFile | null) => void;
  setFileToShare: (file: FileToShare | null) => void;
  setFilesToDelete: (files: BulkFile[] | null) => void;
  setFilesToShare: (files: BulkFile[] | null) => void;
  setFilesToDownload: (files: BulkFile[] | null) => void;
  setBulkDownloadModalOpen: (open: boolean) => void;
  handleDelete: (fileId: string) => Promise<void>;
  handleDownload: (objectName: string, fileName: string) => Promise<void>;
  handleRename: (fileId: string, newName: string, description?: string) => Promise<void>;
  handleBulkDelete: (files: BulkFile[]) => void;
  handleBulkShare: (files: BulkFile[]) => void;
  handleBulkDownload: (files: BulkFile[]) => void;
  handleBulkDownloadWithZip: (files: BulkFile[], zipName: string) => Promise<void>;
  handleDeleteBulk: () => Promise<void>;
  handleShareBulkSuccess: () => void;
  clearSelection?: () => void;
  setClearSelectionCallback?: (callback: () => void) => void;
  getDownloadStatus: (objectName: string) => PendingDownload | null;
  cancelPendingDownload: (downloadId: string) => Promise<void>;
  isDownloadPending: (objectName: string) => boolean;
}

export function useEnhancedFileManager(onRefresh: () => Promise<void>, clearSelection?: () => void) {
  const t = useTranslations();
  const downloadQueue = useDownloadQueue(true, 3000);
  const notifications = usePushNotifications();

  const [previewFile, setPreviewFile] = useState<PreviewFile | null>(null);
  const [fileToRename, setFileToRename] = useState<FileToRename | null>(null);
  const [fileToDelete, setFileToDelete] = useState<FileToDelete | null>(null);
  const [fileToShare, setFileToShare] = useState<FileToShare | null>(null);
  const [filesToDelete, setFilesToDelete] = useState<BulkFile[] | null>(null);
  const [filesToShare, setFilesToShare] = useState<BulkFile[] | null>(null);
  const [filesToDownload, setFilesToDownload] = useState<BulkFile[] | null>(null);
  const [isBulkDownloadModalOpen, setBulkDownloadModalOpen] = useState(false);
  const [pendingDownloads, setPendingDownloads] = useState<PendingDownload[]>([]);
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | null>(null);

  const startActualDownload = async (
    downloadId: string,
    objectName: string,
    fileName: string,
    downloadUrl?: string
  ) => {
    try {
      setPendingDownloads((prev) =>
        prev.map((d) => (d.downloadId === downloadId ? { ...d, status: "downloading" } : d))
      );

      let url = downloadUrl;
      if (!url) {
        const encodedObjectName = encodeURIComponent(objectName);
        const response = await getDownloadUrl(encodedObjectName);
        url = response.data.url;
      }

      const link = document.createElement("a");
      link.href = url;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);

      const wasQueued = pendingDownloads.some((d) => d.downloadId === downloadId);

      if (wasQueued) {
        setPendingDownloads((prev) =>
          prev.map((d) => (d.downloadId === downloadId ? { ...d, status: "completed" } : d))
        );

        const completedDownload = pendingDownloads.find((d) => d.downloadId === downloadId);
        if (completedDownload) {
          const fileSize = completedDownload.startTime ? Date.now() - completedDownload.startTime : undefined;
          await notifications.notifyDownloadComplete(fileName, fileSize);
        }

        setTimeout(() => {
          setPendingDownloads((prev) => prev.filter((d) => d.downloadId !== downloadId));
        }, 5000);
      }

      if (!wasQueued) {
        toast.success(t("files.downloadStart", { fileName }));
      }
    } catch (error: any) {
      const wasQueued = pendingDownloads.some((d) => d.downloadId === downloadId);

      if (wasQueued) {
        setPendingDownloads((prev) => prev.map((d) => (d.downloadId === downloadId ? { ...d, status: "failed" } : d)));

        const errorMessage =
          error?.response?.data?.message || error?.message || t("notifications.downloadFailed.unknownError");
        await notifications.notifyDownloadFailed(fileName, errorMessage);

        setTimeout(() => {
          setPendingDownloads((prev) => prev.filter((d) => d.downloadId !== downloadId));
        }, 10000);
      }

      if (!pendingDownloads.some((d) => d.downloadId === downloadId)) {
        toast.error(t("files.downloadError"));
      }
      throw error;
    }
  };

  useEffect(() => {
    if (!downloadQueue.queueStatus) return;

    pendingDownloads.forEach(async (download) => {
      if (download.status === "queued") {
        const stillQueued = downloadQueue.queueStatus?.queuedDownloads.find((qd) => qd.fileName === download.fileName);

        if (!stillQueued) {
          console.log(`[DOWNLOAD] Processing queued download: ${download.fileName}`);

          await notifications.notifyQueueProcessing(download.fileName);

          await startActualDownload(download.downloadId, download.objectName, download.fileName);
        }
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [downloadQueue.queueStatus, pendingDownloads, notifications]);

  const setClearSelectionCallback = useCallback((callback: () => void) => {
    setClearSelectionCallbackState(() => callback);
  }, []);

  const generateDownloadId = useCallback(() => {
    return `${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;
  }, []);

  const handleDownload = async (objectName: string, fileName: string) => {
    const downloadId = generateDownloadId();

    try {
      const encodedObjectName = encodeURIComponent(objectName);
      const response = await getDownloadUrl(encodedObjectName);

      if (response.status === 202) {
        const pendingDownload: PendingDownload = {
          downloadId,
          fileName,
          objectName,
          startTime: Date.now(),
          status: "queued",
        };

        setPendingDownloads((prev) => [...prev, pendingDownload]);

        toast.info(t("downloadQueue.downloadQueued", { fileName }), {
          description: t("downloadQueue.queuedDescription"),
          duration: 5000,
        });
      } else {
        await startActualDownload(downloadId, objectName, fileName, response.data.url);
      }
    } catch (error: any) {
      setPendingDownloads((prev) => prev.filter((d) => d.downloadId !== downloadId));

      if (error.response?.status === 503) {
        toast.error(t("downloadQueue.queueFull"), {
          description: t("downloadQueue.queueFullDescription"),
        });
      } else {
        toast.error(t("files.downloadError"), {
          description: error.response?.data?.message || error.message,
        });
      }
      throw error;
    }
  };

  const cancelPendingDownload = async (downloadId: string) => {
    try {
      await downloadQueue.cancelDownload(downloadId);
      setPendingDownloads((prev) => prev.filter((d) => d.downloadId !== downloadId));
    } catch (error) {
      console.error("Error cancelling download:", error);
    }
  };

  const getDownloadStatus = useCallback(
    (objectName: string): PendingDownload | null => {
      return pendingDownloads.find((d) => d.objectName === objectName) || null;
    },
    [pendingDownloads]
  );

  const isDownloadPending = useCallback(
    (objectName: string): boolean => {
      return pendingDownloads.some((d) => d.objectName === objectName && d.status !== "completed");
    },
    [pendingDownloads]
  );

  const handleRename = async (fileId: string, newName: string, description?: string) => {
    try {
      await updateFile(fileId, {
        name: newName,
        description: description || null,
      });
      await onRefresh();
      toast.success(t("files.updateSuccess"));
      setFileToRename(null);
    } catch (error) {
      console.error("Failed to update file:", error);
      toast.error(t("files.updateError"));
    }
  };

  const handleDelete = async (fileId: string) => {
    try {
      await deleteFile(fileId);
      await onRefresh();
      toast.success(t("files.deleteSuccess"));
      setFileToDelete(null);
    } catch (error) {
      console.error("Failed to delete file:", error);
      toast.error(t("files.deleteError"));
    }
  };

  const handleBulkDelete = (files: BulkFile[]) => {
    setFilesToDelete(files);
  };

  const handleBulkShare = (files: BulkFile[]) => {
    setFilesToShare(files);
  };

  const handleShareBulkSuccess = () => {
    setFilesToShare(null);
    if (clearSelectionCallback) {
      clearSelectionCallback();
    }
  };

  const handleBulkDownload = (files: BulkFile[]) => {
    setFilesToDownload(files);
    setBulkDownloadModalOpen(true);
  };

  const downloadFileAsBlobSilent = async (objectName: string, fileName: string): Promise<Blob> => {
    try {
      const encodedObjectName = encodeURIComponent(objectName);
      const response = await getDownloadUrl(encodedObjectName);

      if (response.status === 202) {
        const { downloadFileAsBlobWithQueue } = await import("@/utils/download-queue-utils");
        return await downloadFileAsBlobWithQueue(objectName, fileName, false);
      } else {
        const fetchResponse = await fetch(response.data.url);
        if (!fetchResponse.ok) throw new Error(`Failed to download ${fileName}`);
        return await fetchResponse.blob();
      }
    } catch (error: any) {
      throw error;
    }
  };

  const handleBulkDownloadWithZip = async (files: BulkFile[], zipName: string) => {
    try {
      toast.promise(
        (async () => {
          const JSZip = (await import("jszip")).default;
          const zip = new JSZip();

          let bulkDownloadId: string | null = null;
          let shouldShowInQueue = false;

          if (files.length > 0) {
            try {
              const testFile = files[0];
              const encodedObjectName = encodeURIComponent(testFile.objectName);
              const testResponse = await getDownloadUrl(encodedObjectName);

              if (testResponse.status === 202) {
                shouldShowInQueue = true;
              }
            } catch (error) {
              console.error("Error checking if file is queued:", error);
              shouldShowInQueue = true;
            }
          }

          if (shouldShowInQueue) {
            bulkDownloadId = generateDownloadId();
            const bulkPendingDownload: PendingDownload = {
              downloadId: bulkDownloadId,
              fileName: zipName.endsWith(".zip") ? zipName : `${zipName}.zip`,
              objectName: "bulk-download",
              startTime: Date.now(),
              status: "pending",
            };
            setPendingDownloads((prev) => [...prev, bulkPendingDownload]);

            setPendingDownloads((prev) =>
              prev.map((d) => (d.downloadId === bulkDownloadId ? { ...d, status: "downloading" } : d))
            );
          }

          const downloadPromises = files.map(async (file) => {
            try {
              const blob = await downloadFileAsBlobSilent(file.objectName, file.name);
              zip.file(file.name, blob);
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

          if (bulkDownloadId && shouldShowInQueue) {
            setPendingDownloads((prev) =>
              prev.map((d) => (d.downloadId === bulkDownloadId ? { ...d, status: "completed" } : d))
            );

            setTimeout(() => {
              setPendingDownloads((prev) => prev.filter((d) => d.downloadId !== bulkDownloadId));
            }, 5000);
          }

          if (clearSelectionCallback) {
            clearSelectionCallback();
          }
        })(),
        {
          loading: t("shareManager.creatingZip"),
          success: t("shareManager.zipDownloadSuccess"),
          error: t("shareManager.zipDownloadError"),
        }
      );
    } catch (error: any) {
      console.error("Error creating ZIP:", error);
    }
  };

  const handleDeleteBulk = async () => {
    if (!filesToDelete) return;

    try {
      const deletePromises = filesToDelete.map((file) => deleteFile(file.id));
      await Promise.all(deletePromises);

      toast.success(t("files.bulkDeleteSuccess", { count: filesToDelete.length }));
      setFilesToDelete(null);
      onRefresh();

      if (clearSelectionCallback) {
        clearSelectionCallback();
      }
    } catch (error) {
      console.error("Failed to delete files:", error);
      toast.error(t("files.bulkDeleteError"));
    }
  };

  return {
    previewFile,
    setPreviewFile,
    fileToRename,
    setFileToRename,
    fileToDelete,
    setFileToDelete,
    fileToShare,
    setFileToShare,
    filesToDelete,
    setFilesToDelete,
    filesToShare,
    setFilesToShare,
    filesToDownload,
    setFilesToDownload,
    isBulkDownloadModalOpen,
    setBulkDownloadModalOpen,
    pendingDownloads,
    handleDownload,
    handleRename,
    handleDelete,
    handleBulkDelete,
    handleBulkShare,
    handleBulkDownload,
    handleBulkDownloadWithZip,
    handleDeleteBulk,
    handleShareBulkSuccess,
    clearSelection,
    setClearSelectionCallback,
    getDownloadStatus,
    cancelPendingDownload,
    isDownloadPending,
  };
}
