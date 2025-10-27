import { useCallback, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { deleteFile, updateFile } from "@/http/endpoints";
import { deleteFolder, registerFolder, updateFolder } from "@/http/endpoints/folders";
import { getCachedDownloadUrl } from "@/lib/download-url-cache";

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
  folderId?: string;
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
}

export function useEnhancedFileManager(
  onRefresh: () => Promise<void>,
  clearSelection?: () => void,
  handleImmediateUpdate?: (itemId: string, itemType: "file" | "folder", newParentId: string | null) => void,
  allFiles?: BulkFile[],
  allFolders?: BulkFolder[]
) {
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
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | null>(null);

  const [foldersToShare, setFoldersToShare] = useState<BulkFolder[] | null>(null);
  const [foldersToDownload, setFoldersToDownload] = useState<BulkFolder[] | null>(null);

  const setClearSelectionCallback = useCallback((callback: () => void) => {
    setClearSelectionCallbackState(() => callback);
  }, []);

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
      const loadingToast = toast.loading(t("share.messages.downloadStarted"));
      const url = await getCachedDownloadUrl(objectName);
      const link = document.createElement("a");
      link.href = url;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);

      toast.dismiss(loadingToast);
      toast.success(t("shareManager.downloadSuccess"));
    } catch (error) {
      console.error(`[FileManager] ❌ Download failed for ${fileName}:`, error);
      toast.error(t("share.errors.downloadFailed"));
    }
  };

  const handleRename = async (fileId: string, newName: string, description?: string) => {
    try {
      await updateFile(fileId, {
        name: newName,
        description: description || null,
      });
      toast.success(t("files.updateSuccess"));
      setFileToRename(null);
    } catch (error) {
      console.error("Failed to update file:", error);
      toast.error(t("files.updateError"));
    }
  };

  const handleDelete = async (fileId: string) => {
    try {
      // Optimistic update - remove from UI immediately
      if (handleImmediateUpdate) {
        handleImmediateUpdate(fileId, "file", "__DELETE__" as any);
      }

      await deleteFile(fileId);
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

  const handleBulkDownloadWithZip = async (files: BulkFile[], zipName: string) => {
    try {
      const folders = foldersToDownload || [];

      if (files.length === 0 && folders.length === 0) {
        toast.error(t("shareManager.noFilesToDownload"));
        return;
      }

      const loadingToast = toast.loading(t("shareManager.creatingZip"));

      try {
        // Collect all files including those in folders recursively
        const allFilesToDownload: Array<{ url: string; name: string }> = [];

        // Helper function to get all files in a folder recursively with paths
        const getFolderFilesWithPath = (
          targetFolderId: string,
          currentPath: string = ""
        ): Array<{ file: BulkFile; path: string }> => {
          if (!allFiles || !allFolders) return [];

          const filesWithPath: Array<{ file: BulkFile; path: string }> = [];

          // Get direct files in this folder
          const directFiles = allFiles.filter((f) => f.folderId === targetFolderId);
          directFiles.forEach((file) => {
            filesWithPath.push({ file, path: currentPath });
          });

          // Get subfolders and process them recursively
          const subfolders = allFolders.filter((f) => f.parentId === targetFolderId);
          for (const subfolder of subfolders) {
            const subfolderPath = currentPath ? `${currentPath}/${subfolder.name}` : subfolder.name;
            filesWithPath.push(...getFolderFilesWithPath(subfolder.id, subfolderPath));
          }

          return filesWithPath;
        };

        // Get presigned URLs for direct files (not in folders)
        const directFileItems = await Promise.all(
          files.map(async (file) => {
            const url = await getCachedDownloadUrl(file.objectName);
            return {
              url,
              name: file.name,
            };
          })
        );
        allFilesToDownload.push(...directFileItems);

        // Get presigned URLs for files in selected folders
        for (const folder of folders) {
          const folderFilesWithPath = getFolderFilesWithPath(folder.id, folder.name);

          const folderFileItems = await Promise.all(
            folderFilesWithPath.map(async ({ file, path }) => {
              const url = await getCachedDownloadUrl(file.objectName);
              return {
                url,
                name: path ? `${path}/${file.name}` : file.name,
              };
            })
          );
          allFilesToDownload.push(...folderFileItems);
        }

        if (allFilesToDownload.length === 0) {
          toast.dismiss(loadingToast);
          toast.error(t("shareManager.noFilesToDownload"));
          return;
        }

        // Create ZIP with all files
        const { downloadFilesAsZip } = await import("@/utils/zip-download");
        await downloadFilesAsZip(allFilesToDownload, zipName.endsWith(".zip") ? zipName : `${zipName}.zip`);

        toast.dismiss(loadingToast);
        toast.success(t("shareManager.zipDownloadSuccess"));
      } catch (error) {
        toast.dismiss(loadingToast);
        toast.error(t("shareManager.zipDownloadError"));
        throw error;
      }

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
    }
  };

  const handleDeleteBulk = async () => {
    if (!filesToDelete && !foldersToDelete) return;

    try {
      // Optimistic update - remove all items from UI immediately
      if (handleImmediateUpdate) {
        filesToDelete?.forEach((file) => {
          handleImmediateUpdate(file.id, "file", "__DELETE__" as any);
        });
        foldersToDelete?.forEach((folder) => {
          handleImmediateUpdate(folder.id, "folder", "__DELETE__" as any);
        });
      }

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
      setFolderToRename(null);
    } catch (error) {
      console.error("Error renaming folder:", error);
      toast.error(t("folderActions.renameFolderError"));
    }
  };

  const handleFolderDelete = async (folderId: string) => {
    try {
      // Optimistic update - remove from UI immediately
      if (handleImmediateUpdate) {
        handleImmediateUpdate(folderId, "folder", "__DELETE__" as any);
      }

      await deleteFolder(folderId);
      toast.success(t("folderActions.folderDeleted"));
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
  };
}
