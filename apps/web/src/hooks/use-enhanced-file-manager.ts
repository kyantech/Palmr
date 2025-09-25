import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { deleteFile, getDownloadUrl, updateFile } from "@/http/endpoints";
import { bulkDownloadFiles, downloadFolder } from "@/http/endpoints/bulk-download";
import { deleteFolder, registerFolder, updateFolder } from "@/http/endpoints/folders";

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

  folderToDelete: FolderToDelete | null;
  folderToRename: FolderToRename | null;
  folderToShare: FolderToShare | null;
  isCreateFolderModalOpen: boolean;

  foldersToShare: BulkFolder[] | null;
  foldersToDownload: BulkFolder[] | null;

  setFileToDelete: (file: any) => void;
  setFileToRename: (file: any) => void;
  setPreviewFile: (file: PreviewFile | null) => void;
  setFileToShare: (file: FileToShare | null) => void;
  setFilesToDelete: (files: BulkFile[] | null) => void;
  setFilesToShare: (files: BulkFile[] | null) => void;
  setFilesToDownload: (files: BulkFile[] | null) => void;
  setFoldersToDelete: (folders: BulkFolder[] | null) => void;
  setBulkDownloadModalOpen: (open: boolean) => void;

  setFolderToDelete: (folder: FolderToDelete | null) => void;
  setFolderToRename: (folder: FolderToRename | null) => void;
  setFolderToShare: (folder: FolderToShare | null) => void;
  setCreateFolderModalOpen: (open: boolean) => void;
  setFoldersToShare: (folders: BulkFolder[] | null) => void;
  setFoldersToDownload: (folders: BulkFolder[] | null) => void;

  handleDelete: (fileId: string) => Promise<void>;
  handleDownload: (objectName: string, fileName: string) => Promise<void>;
  handleRename: (fileId: string, newName: string, description?: string) => Promise<void>;
  handleBulkDelete: (files: BulkFile[], folders?: BulkFolder[]) => void;
  handleBulkShare: (files: BulkFile[], folders?: BulkFolder[]) => void;
  handleBulkDownload: (files: BulkFile[], folders?: BulkFolder[]) => void;
  handleBulkDownloadWithZip: (files: BulkFile[], zipName: string) => Promise<void>;
  handleDeleteBulk: () => Promise<void>;
  handleShareBulkSuccess: () => void;

  handleCreateFolder: (data: { name: string; description?: string }, parentId?: string) => Promise<void>;
  handleFolderDelete: (folderId: string) => Promise<void>;
  handleFolderRename: (folderId: string, newName: string, description?: string) => Promise<void>;

  clearSelection?: () => void;
  setClearSelectionCallback?: (callback: () => void) => void;
  getDownloadStatus: (objectName: string) => PendingDownload | null;
  cancelPendingDownload: (downloadId: string) => Promise<void>;
  isDownloadPending: (objectName: string) => boolean;
}

export function useEnhancedFileManager(onRefresh: () => Promise<void>, clearSelection?: () => void) {
  const t = useTranslations();

  const [previewFile, setPreviewFile] = useState<PreviewFile | null>(null);
  const [fileToRename, setFileToRename] = useState<FileToRename | null>(null);
  const [fileToDelete, setFileToDelete] = useState<FileToDelete | null>(null);
  const [fileToShare, setFileToShare] = useState<FileToShare | null>(null);
  const [filesToDelete, setFilesToDelete] = useState<BulkFile[] | null>(null);
  const [filesToShare, setFilesToShare] = useState<BulkFile[] | null>(null);
  const [filesToDownload, setFilesToDownload] = useState<BulkFile[] | null>(null);
  const [foldersToDelete, setFoldersToDelete] = useState<BulkFolder[] | null>(null);

  const [folderToDelete, setFolderToDelete] = useState<FolderToDelete | null>(null);
  const [folderToRename, setFolderToRename] = useState<FolderToRename | null>(null);
  const [folderToShare, setFolderToShare] = useState<FolderToShare | null>(null);
  const [isCreateFolderModalOpen, setCreateFolderModalOpen] = useState(false);
  const [isBulkDownloadModalOpen, setBulkDownloadModalOpen] = useState(false);
  const [pendingDownloads, setPendingDownloads] = useState<PendingDownload[]>([]);
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | null>(null);

  const [foldersToShare, setFoldersToShare] = useState<BulkFolder[] | null>(null);
  const [foldersToDownload, setFoldersToDownload] = useState<BulkFolder[] | null>(null);

  const setClearSelectionCallback = useCallback((callback: () => void) => {
    setClearSelectionCallbackState(() => callback);
  }, []);

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
      const encodedObjectName = encodeURIComponent(objectName);
      const response = await getDownloadUrl(encodedObjectName);

      // Direct S3 download - no queue needed
      const link = document.createElement("a");
      link.href = response.data.url;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);

      toast.success(t("shareManager.downloadSuccess"));
    } catch (error) {
      console.error("Download error:", error);
      toast.error(t("share.errors.downloadFailed"));
    }
  };

  const cancelPendingDownload = async (downloadId: string) => {
    // Queue functionality removed - just remove from local state
    setPendingDownloads((prev) => prev.filter((d) => d.downloadId !== downloadId));
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

    if (clearSelectionCallback) {
      clearSelectionCallback();
    }
  };

  const handleSingleFolderDownload = async (folderId: string, folderName: string) => {
    try {
      // Show creating ZIP toast
      const creatingToast = toast.loading(t("bulkDownload.creatingZip"));

      const blob = await downloadFolder(folderId, folderName);

      // Update toast to success
      toast.dismiss(creatingToast);
      toast.success(t("bulkDownload.zipCreated"));

      // Create download link
      const url = window.URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `${folderName}.zip`;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      window.URL.revokeObjectURL(url);
    } catch (error) {
      console.error("Folder download error:", error);
      toast.error(t("bulkDownload.zipError"));
    }
  };

  const handleBulkDownloadWithZip = async (files: BulkFile[], zipName: string) => {
    try {
      const folders = foldersToDownload || [];

      if (files.length === 0 && folders.length === 0) {
        toast.error(t("shareManager.noFilesToDownload"));
        return;
      }

      const fileIds = files.map((file) => file.id);
      const folderIds = folders.map((folder) => folder.id);

      // Show creating ZIP toast
      const creatingToast = toast.loading(t("bulkDownload.creatingZip"));

      const blob = await bulkDownloadFiles({
        fileIds,
        folderIds,
        zipName,
      });

      // Update toast to success
      toast.dismiss(creatingToast);
      toast.success(t("bulkDownload.zipCreated"));

      // Create download link
      const url = window.URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = zipName;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      window.URL.revokeObjectURL(url);

      setBulkDownloadModalOpen(false);
      setFilesToDownload(null);
      setFoldersToDownload(null);

      if (clearSelectionCallback) {
        clearSelectionCallback();
      }
    } catch (error) {
      console.error("Error in bulk download:", error);
      setBulkDownloadModalOpen(false);
      setFilesToDownload(null);
      setFoldersToDownload(null);
      toast.error(t("bulkDownload.zipError"));
    }
  };

  const handleDeleteBulk = async () => {
    if (!filesToDelete && !foldersToDelete) return;

    try {
      const deletePromises = [];

      if (filesToDelete) {
        deletePromises.push(...filesToDelete.map((file) => deleteFile(file.id)));
      }

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

    foldersToShare,
    setFoldersToShare,
    foldersToDownload,
    setFoldersToDownload,

    clearSelection,
    setClearSelectionCallback,
    getDownloadStatus,
    handleSingleFolderDownload,
    cancelPendingDownload,
    isDownloadPending,
  };
}
