import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { deleteFile, getDownloadUrl, updateFile } from "@/http/endpoints";
import { deleteFolder, registerFolder, updateFolder } from "@/http/endpoints/folders";
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

interface FolderToRename {
  id: string;
  name: string;
  description?: string;
}

interface FolderToDelete {
  id: string;
  name: string;
}

interface FolderToShare {
  id: string;
  name: string;
  description?: string;
  objectName: string;
  parentId?: string;
  userId: string;
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
  relativePath?: string;
}

interface BulkFolder {
  id: string;
  name: string;
  description?: string;
  objectName: string;
  parentId?: string;
  userId: string;
  createdAt: string;
  updatedAt: string;
  totalSize?: string;
  _count?: {
    files: number;
    children: number;
  };
}

interface PendingDownload {
  downloadId: string;
  fileName: string;
  objectName: string;
  startTime: number;
  status: "pending" | "queued" | "downloading" | "completed" | "failed";
}

export interface EnhancedFileManagerHook {
  // File states
  previewFile: PreviewFile | null;
  fileToDelete: any;
  fileToRename: any;
  fileToShare: FileToShare | null;
  filesToDelete: BulkFile[] | null;
  filesToShare: BulkFile[] | null;
  filesToDownload: BulkFile[] | null;
  foldersToDelete: BulkFolder[] | null;
  isBulkDownloadModalOpen: boolean;
  pendingDownloads: PendingDownload[];

  // Folder states
  folderToDelete: FolderToDelete | null;
  folderToRename: FolderToRename | null;
  folderToShare: FolderToShare | null;
  isCreateFolderModalOpen: boolean;

  // Combined bulk states
  foldersToShare: BulkFolder[] | null;
  foldersToDownload: BulkFolder[] | null;

  // File setters
  setFileToDelete: (file: any) => void;
  setFileToRename: (file: any) => void;
  setPreviewFile: (file: PreviewFile | null) => void;
  setFileToShare: (file: FileToShare | null) => void;
  setFilesToDelete: (files: BulkFile[] | null) => void;
  setFilesToShare: (files: BulkFile[] | null) => void;
  setFilesToDownload: (files: BulkFile[] | null) => void;
  setFoldersToDelete: (folders: BulkFolder[] | null) => void;
  setBulkDownloadModalOpen: (open: boolean) => void;

  // Folder setters
  setFolderToDelete: (folder: FolderToDelete | null) => void;
  setFolderToRename: (folder: FolderToRename | null) => void;
  setFolderToShare: (folder: FolderToShare | null) => void;
  setCreateFolderModalOpen: (open: boolean) => void;
  setFoldersToShare: (folders: BulkFolder[] | null) => void;
  setFoldersToDownload: (folders: BulkFolder[] | null) => void;

  // File handlers
  handleDelete: (fileId: string) => Promise<void>;
  handleDownload: (objectName: string, fileName: string) => Promise<void>;
  handleRename: (fileId: string, newName: string, description?: string) => Promise<void>;
  handleBulkDelete: (files: BulkFile[], folders?: BulkFolder[]) => void;
  handleBulkShare: (files: BulkFile[], folders?: BulkFolder[]) => void;
  handleBulkDownload: (files: BulkFile[], folders?: BulkFolder[]) => void;
  handleBulkDownloadWithZip: (files: BulkFile[], zipName: string) => Promise<void>;
  handleDeleteBulk: () => Promise<void>;
  handleShareBulkSuccess: () => void;

  // Folder handlers
  handleCreateFolder: (data: { name: string; description?: string }, parentId?: string) => Promise<void>;
  handleFolderDelete: (folderId: string) => Promise<void>;
  handleFolderRename: (folderId: string, newName: string, description?: string) => Promise<void>;

  // Common
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

  // File states
  const [previewFile, setPreviewFile] = useState<PreviewFile | null>(null);
  const [fileToRename, setFileToRename] = useState<FileToRename | null>(null);
  const [fileToDelete, setFileToDelete] = useState<FileToDelete | null>(null);
  const [fileToShare, setFileToShare] = useState<FileToShare | null>(null);
  const [filesToDelete, setFilesToDelete] = useState<BulkFile[] | null>(null);
  const [filesToShare, setFilesToShare] = useState<BulkFile[] | null>(null);
  const [filesToDownload, setFilesToDownload] = useState<BulkFile[] | null>(null);
  const [foldersToDelete, setFoldersToDelete] = useState<BulkFolder[] | null>(null);

  // Folder states
  const [folderToDelete, setFolderToDelete] = useState<FolderToDelete | null>(null);
  const [folderToRename, setFolderToRename] = useState<FolderToRename | null>(null);
  const [folderToShare, setFolderToShare] = useState<FolderToShare | null>(null);
  const [isCreateFolderModalOpen, setCreateFolderModalOpen] = useState(false);
  const [isBulkDownloadModalOpen, setBulkDownloadModalOpen] = useState(false);
  const [pendingDownloads, setPendingDownloads] = useState<PendingDownload[]>([]);
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | null>(null);

  // Combined bulk states
  const [foldersToShare, setFoldersToShare] = useState<BulkFolder[] | null>(null);
  const [foldersToDownload, setFoldersToDownload] = useState<BulkFolder[] | null>(null);

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

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
      // Use the original download pattern for consistency with promise-based toast
      const { downloadFileWithQueue } = await import("@/utils/download-queue-utils");

      await toast.promise(
        downloadFileWithQueue(objectName, fileName, {
          silent: true,
          showToasts: false,
        }),
        {
          loading: t("share.messages.downloadStarted"),
          success: t("shareManager.downloadSuccess"),
          error: t("share.errors.downloadFailed"),
        }
      );
    } catch (error) {
      console.error("Download error:", error);
      // Error already handled in toast.promise
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

  const handleBulkDelete = (files: BulkFile[], folders?: BulkFolder[]) => {
    setFilesToDelete(files.length > 0 ? files : null);
    setFoldersToDelete(folders && folders.length > 0 ? folders : null);
  };

  const handleBulkShare = (files: BulkFile[], folders?: BulkFolder[]) => {
    setFilesToShare(files);
    setFoldersToShare(folders || null);
  };

  const handleShareBulkSuccess = () => {
    setFilesToShare(null);
    setFoldersToShare(null);
    if (clearSelectionCallback) {
      clearSelectionCallback();
    }
  };

  const handleBulkDownload = (files: BulkFile[], folders?: BulkFolder[]) => {
    setFilesToDownload(files);
    setFoldersToDownload(folders || null);
    setBulkDownloadModalOpen(true);

    // Clear selection immediately when bulk download modal opens
    if (clearSelectionCallback) {
      clearSelectionCallback();
    }
  };

  const handleSingleFolderDownload = async (folderId: string, folderName: string) => {
    try {
      // Use the enhanced download queue utility for folders with promise-based toast
      const { downloadFolderWithQueue } = await import("@/utils/download-queue-utils");

      await toast.promise(
        downloadFolderWithQueue(folderId, folderName, {
          silent: true,
          showToasts: false,
        }),
        {
          loading: t("shareManager.creatingZip"),
          success: t("shareManager.zipDownloadSuccess"),
          error: t("share.errors.downloadFailed"),
        }
      );
    } catch (error) {
      console.error("Error downloading folder:", error);
      // Error already handled in toast.promise
    }
  };

  const handleBulkDownloadWithZip = async (files: BulkFile[], zipName: string) => {
    try {
      const folders = foldersToDownload || [];
      const { bulkDownloadWithQueue } = await import("@/utils/download-queue-utils");

      // Prepare items for the enhanced bulk download utility
      const allItems = [
        // Add individual files
        ...files.map((file) => ({
          objectName: file.objectName,
          name: file.relativePath || file.name,
          isReverseShare: false,
          type: "file" as const,
        })),
        // Add folders
        ...folders.map((folder) => ({
          id: folder.id,
          name: folder.name,
          type: "folder" as const,
        })),
      ];

      if (allItems.length === 0) {
        toast.error(t("shareManager.noFilesToDownload"));
        return;
      }

      toast.promise(
        bulkDownloadWithQueue(allItems, zipName, undefined, false).then(() => {
          setBulkDownloadModalOpen(false);
          setFilesToDownload(null);
          setFoldersToDownload(null);
          if (clearSelectionCallback) {
            clearSelectionCallback();
          }
        }),
        {
          loading: t("shareManager.creatingZip"),
          success: t("shareManager.zipDownloadSuccess"),
          error: t("shareManager.zipDownloadError"),
        }
      );
    } catch (error) {
      console.error("Error in bulk download:", error);
      setBulkDownloadModalOpen(false);
      setFilesToDownload(null);
      setFoldersToDownload(null);
    }
  };

  const handleDeleteBulk = async () => {
    if (!filesToDelete && !foldersToDelete) return;

    try {
      const deletePromises = [];

      // Delete files
      if (filesToDelete) {
        deletePromises.push(...filesToDelete.map((file) => deleteFile(file.id)));
      }

      // Delete folders
      if (foldersToDelete) {
        deletePromises.push(...foldersToDelete.map((folder) => deleteFolder(folder.id)));
      }

      await Promise.all(deletePromises);

      const totalCount = (filesToDelete?.length || 0) + (foldersToDelete?.length || 0);
      toast.success(t("files.bulkDeleteSuccess", { count: totalCount }));
      setFilesToDelete(null);
      setFoldersToDelete(null);
      onRefresh();
    } catch (error) {
      console.error("Failed to delete items:", error);
      toast.error(t("files.bulkDeleteError"));
    }
  };

  // Folder handlers - following the same pattern as file handlers
  const handleCreateFolder = async (data: { name: string; description?: string }, parentId?: string) => {
    try {
      const folderData = {
        name: data.name,
        description: data.description,
        objectName: `folders/${Date.now()}-${data.name}`,
        parentId: parentId || undefined,
      };

      await registerFolder(folderData);
      toast.success(t("folderActions.folderCreated"));
      await onRefresh();
      setCreateFolderModalOpen(false);
    } catch (error) {
      console.error("Error creating folder:", error);
      toast.error(t("folderActions.createFolderError"));
      throw error;
    }
  };

  const handleFolderRename = async (folderId: string, newName: string, description?: string) => {
    try {
      await updateFolder(folderId, { name: newName, description });
      toast.success(t("folderActions.folderRenamed"));
      await onRefresh();
      setFolderToRename(null);
    } catch (error) {
      console.error("Error renaming folder:", error);
      toast.error(t("folderActions.renameFolderError"));
    }
  };

  const handleFolderDelete = async (folderId: string) => {
    try {
      await deleteFolder(folderId);
      toast.success(t("folderActions.folderDeleted"));
      await onRefresh();
      setFolderToDelete(null);
      if (clearSelectionCallback) {
        clearSelectionCallback();
      }
    } catch (error) {
      console.error("Error deleting folder:", error);
      toast.error(t("folderActions.deleteFolderError"));
    }
  };

  return {
    // File states and handlers
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
    foldersToDelete,
    setFoldersToDelete,
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

    // Folder states and handlers
    folderToDelete,
    setFolderToDelete,
    folderToRename,
    setFolderToRename,
    folderToShare,
    setFolderToShare,
    isCreateFolderModalOpen,
    setCreateFolderModalOpen,
    handleCreateFolder,
    handleFolderRename,
    handleFolderDelete,

    // Combined bulk states
    foldersToShare,
    setFoldersToShare,
    foldersToDownload,
    setFoldersToDownload,

    // Common
    clearSelection,
    setClearSelectionCallback,
    getDownloadStatus,
    handleSingleFolderDownload,
    cancelPendingDownload,
    isDownloadPending,
  };
}
