import { useCallback, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { deleteFile, getDownloadUrl, updateFile } from "@/http/endpoints";

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

export interface FileManagerHook {
  previewFile: PreviewFile | null;
  fileToDelete: any;
  fileToRename: any;
  fileToShare: FileToShare | null;
  filesToDelete: BulkFile[] | null;
  filesToShare: BulkFile[] | null;
  filesToDownload: BulkFile[] | null;
  isBulkDownloadModalOpen: boolean;
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
}

export function useFileManager(onRefresh: () => Promise<void>, clearSelection?: () => void) {
  const t = useTranslations();
  const [previewFile, setPreviewFile] = useState<PreviewFile | null>(null);
  const [fileToRename, setFileToRename] = useState<FileToRename | null>(null);
  const [fileToDelete, setFileToDelete] = useState<FileToDelete | null>(null);
  const [fileToShare, setFileToShare] = useState<FileToShare | null>(null);
  const [filesToDelete, setFilesToDelete] = useState<BulkFile[] | null>(null);
  const [filesToShare, setFilesToShare] = useState<BulkFile[] | null>(null);
  const [filesToDownload, setFilesToDownload] = useState<BulkFile[] | null>(null);
  const [isBulkDownloadModalOpen, setBulkDownloadModalOpen] = useState(false);
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | null>(null);

  const setClearSelectionCallback = useCallback((callback: () => void) => {
    setClearSelectionCallbackState(() => callback);
  }, []);

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
      const encodedObjectName = encodeURIComponent(objectName);
      const response = await getDownloadUrl(encodedObjectName);
      const downloadUrl = response.data.url;

      const link = document.createElement("a");
      link.href = downloadUrl;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      toast.success(t("files.downloadStart"));
    } catch (error) {
      toast.error(t("files.downloadError"));
      throw error;
    }
  };

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
      toast.success(t("files.updateError"));
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
      toast.success(t("files.deleteError"));
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

  const handleBulkDownloadWithZip = async (files: BulkFile[], zipName: string) => {
    try {
      toast.promise(
        (async () => {
          const JSZip = (await import("jszip")).default;
          const zip = new JSZip();

          const downloadPromises = files.map(async (file) => {
            try {
              const encodedObjectName = encodeURIComponent(file.objectName);
              const downloadResponse = await getDownloadUrl(encodedObjectName);
              const downloadUrl = downloadResponse.data.url;
              const response = await fetch(downloadUrl);

              if (!response.ok) {
                throw new Error(`Failed to download ${file.name}`);
              }

              const blob = await response.blob();
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
    } catch (error) {
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
    setClearSelectionCallback: setClearSelectionCallback,
  };
}
