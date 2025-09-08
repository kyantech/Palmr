"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getShareByAlias } from "@/http/endpoints/index";
import type { Share } from "@/http/endpoints/shares/types";
import {
  bulkDownloadShareWithQueue,
  downloadFileWithQueue,
  downloadShareFolderWithQueue,
} from "@/utils/download-queue-utils";

interface ShareBrowseState {
  folders: any[];
  files: any[];
  path: any[];
  isLoading: boolean;
  error: string | null;
}

export function usePublicShare() {
  const t = useTranslations();
  const params = useParams();
  const alias = params?.alias as string;
  const [share, setShare] = useState<Share | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [password, setPassword] = useState("");
  const [isPasswordModalOpen, setIsPasswordModalOpen] = useState(false);
  const [isPasswordError, setIsPasswordError] = useState(false);

  // Browse state for folder navigation
  const [browseState, setBrowseState] = useState<ShareBrowseState>({
    folders: [],
    files: [],
    path: [],
    isLoading: true,
    error: null,
  });
  const [currentFolderId, setCurrentFolderId] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");

  const loadShare = useCallback(
    async (sharePassword?: string) => {
      if (!alias) return;

      const handleShareError = (error: any) => {
        if (error.response?.data?.error === "Password required") {
          setIsPasswordModalOpen(true);
          setShare(null);
        } else if (error.response?.data?.error === "Invalid password") {
          setIsPasswordError(true);
          toast.error(t("share.errors.invalidPassword"));
        } else {
          toast.error(t("share.errors.loadFailed"));
        }
      };

      try {
        setIsLoading(true);
        const response = await getShareByAlias(alias, sharePassword ? { password: sharePassword } : undefined);

        setShare(response.data.share);
        setIsPasswordModalOpen(false);
        setIsPasswordError(false);

        // Load initial folder contents after setting share - will be called by useEffect
      } catch (error: any) {
        handleShareError(error);
      } finally {
        setIsLoading(false);
      }
    },
    [alias]
  );

  const loadFolderContents = useCallback(
    (folderId: string | null) => {
      try {
        setBrowseState((prev) => ({ ...prev, isLoading: true, error: null }));

        if (!share) {
          setBrowseState((prev) => ({
            ...prev,
            isLoading: false,
            error: "No share data available",
          }));
          return;
        }

        // Build hierarchical structure client-side
        const allFiles = share.files || [];
        const allFolders = share.folders || [];

        // Create a set of all folder IDs that are in the share for quick lookup
        const shareFolderIds = new Set(allFolders.map((f) => f.id));

        // Filter for current folder level
        // If we're at root level (folderId === null), include:
        // 1. Folders with no parentId (true root folders)
        // 2. Folders whose parent is NOT in the share (orphaned folders)
        const folders = allFolders.filter((folder: any) => {
          if (folderId === null) {
            // At root level: show folders with no parent OR folders whose parent isn't in the share
            return !folder.parentId || !shareFolderIds.has(folder.parentId);
          } else {
            // At specific folder level: show direct children
            return folder.parentId === folderId;
          }
        });
        const files = allFiles.filter((file: any) => (file.folderId || null) === folderId);

        // Build breadcrumb path
        const path = [];
        if (folderId) {
          let currentId = folderId;
          while (currentId) {
            const folder = allFolders.find((f: any) => f.id === currentId);
            if (folder) {
              path.unshift(folder);
              currentId = (folder as any).parentId;
            } else {
              break;
            }
          }
        }

        setBrowseState({
          folders,
          files,
          path,
          isLoading: false,
          error: null,
        });
      } catch (error: any) {
        console.error("Error loading folder contents:", error);
        setBrowseState((prev) => ({
          ...prev,
          isLoading: false,
          error: "Failed to load folder contents",
        }));
      }
    },
    [share]
  );

  const navigateToFolder = useCallback(
    (folderId?: string) => {
      const targetFolderId = folderId || null;
      setCurrentFolderId(targetFolderId);
      loadFolderContents(targetFolderId);
    },
    [loadFolderContents]
  );

  const handleSearch = useCallback((query: string) => {
    setSearchQuery(query);
  }, []);

  const handlePasswordSubmit = async () => {
    await loadShare(password);
  };

  const handleFolderDownload = async (folderId: string, folderName: string) => {
    try {
      if (!share) {
        throw new Error("Share data not available");
      }

      // Use the share-specific download function with share data
      await downloadShareFolderWithQueue(folderId, folderName, share.files || [], share.folders || [], {
        silent: true,
        showToasts: false,
      });
    } catch (error) {
      console.error("Error downloading folder:", error);
      throw error; // Re-throw so toast.promise can handle it
    }
  };

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
      // Check if this is a folder download request
      if (objectName.startsWith("folder:")) {
        const folderId = objectName.replace("folder:", "");
        await toast.promise(handleFolderDownload(folderId, fileName), {
          loading: t("shareManager.creatingZip"),
          success: t("shareManager.zipDownloadSuccess"),
          error: t("share.errors.downloadFailed"),
        });
      } else {
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
      }
    } catch {
      // Error already handled in toast.promise
    }
  };

  const handleBulkDownload = async () => {
    const totalFiles = share?.files?.length || 0;
    const totalFolders = share?.folders?.length || 0;

    if (totalFiles === 0 && totalFolders === 0) {
      toast.error(t("shareManager.noFilesToDownload"));
      return;
    }

    if (!share) {
      toast.error(t("share.errors.loadFailed"));
      return;
    }

    try {
      const zipName = `${share.name || t("shareManager.defaultShareName")}.zip`;

      // Prepare all items for the share-specific bulk download
      const allItems: Array<{
        objectName?: string;
        name: string;
        id?: string;
        type?: "file" | "folder";
      }> = [];

      // Add only root-level files (files not in any folder)
      if (share.files) {
        share.files.forEach((file) => {
          // Only include files that are not in any folder
          if (!file.folderId) {
            allItems.push({
              objectName: file.objectName,
              name: file.name,
              type: "file",
            });
          }
        });
      }

      // Add only top-level folders (folders that don't have a parent or whose parent isn't in the share)
      if (share.folders) {
        const folderIds = new Set(share.folders.map((f) => f.id));
        share.folders.forEach((folder) => {
          // Only include folders that are at the root level or whose parent isn't also being downloaded
          if (!folder.parentId || !folderIds.has(folder.parentId)) {
            allItems.push({
              id: folder.id,
              name: folder.name,
              type: "folder",
            });
          }
        });
      }

      if (allItems.length === 0) {
        toast.error(t("shareManager.noFilesToDownload"));
        return;
      }

      toast.promise(
        bulkDownloadShareWithQueue(allItems, share.files || [], share.folders || [], zipName, undefined, true).then(
          () => {
            // Success handled by toast.promise
          }
        ),
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

  const handleSelectedItemsBulkDownload = async (files: any[], folders: any[]) => {
    if (files.length === 0 && folders.length === 0) {
      toast.error(t("shareManager.noFilesToDownload"));
      return;
    }

    if (!share) {
      toast.error(t("share.errors.loadFailed"));
      return;
    }

    try {
      // Get all file IDs that belong to selected folders
      const filesInSelectedFolders = new Set<string>();
      for (const folder of folders) {
        const folderFiles = share.files?.filter((f) => f.folderId === folder.id) || [];
        folderFiles.forEach((f) => filesInSelectedFolders.add(f.id));

        // Also check nested folders recursively
        const checkNestedFolders = (parentId: string) => {
          const nestedFolders = share.folders?.filter((f) => f.parentId === parentId) || [];
          for (const nestedFolder of nestedFolders) {
            const nestedFiles = share.files?.filter((f) => f.folderId === nestedFolder.id) || [];
            nestedFiles.forEach((f) => filesInSelectedFolders.add(f.id));
            checkNestedFolders(nestedFolder.id);
          }
        };
        checkNestedFolders(folder.id);
      }

      const allItems = [
        // Add individual files that are NOT in selected folders
        ...files
          .filter((file) => !filesInSelectedFolders.has(file.id))
          .map((file) => ({
            objectName: file.objectName,
            name: file.name,
            type: "file" as const,
          })),
        // Add only top-level folders (avoid duplicating nested folders)
        ...folders
          .filter((folder) => {
            // Only include folders whose parent isn't also selected
            return !folder.parentId || !folders.some((f) => f.id === folder.parentId);
          })
          .map((folder) => ({
            id: folder.id,
            name: folder.name,
            type: "folder" as const,
          })),
      ];

      const zipName = `${share.name || t("shareManager.defaultShareName")}-selected.zip`;

      toast.promise(
        bulkDownloadShareWithQueue(allItems, share.files || [], share.folders || [], zipName, undefined, false).then(
          () => {
            // Success handled by toast.promise
          }
        ),
        {
          loading: t("shareManager.creatingZip"),
          success: t("shareManager.zipDownloadSuccess"),
          error: t("shareManager.zipDownloadError"),
        }
      );
    } catch (error) {
      console.error("Error creating ZIP:", error);
      toast.error(t("shareManager.zipDownloadError"));
    }
  };

  // Filter content based on search query
  const filteredFolders = browseState.folders.filter((folder) =>
    folder.name?.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const filteredFiles = browseState.files.filter((file) =>
    file.name?.toLowerCase().includes(searchQuery.toLowerCase())
  );

  useEffect(() => {
    if (alias) {
      loadShare();
    }
    // Only run on mount or when alias changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [alias]);

  useEffect(() => {
    if (share) {
      loadFolderContents(null);
    }
  }, [share, loadFolderContents]);

  return {
    // Original functionality
    isLoading,
    share,
    password,
    isPasswordModalOpen,
    isPasswordError,
    setPassword,
    handlePasswordSubmit,
    handleDownload,
    handleBulkDownload,
    handleSelectedItemsBulkDownload,

    // Browse functionality
    folders: filteredFolders,
    files: filteredFiles,
    path: browseState.path,
    isBrowseLoading: browseState.isLoading,
    browseError: browseState.error,
    currentFolderId,
    searchQuery,
    navigateToFolder,
    handleSearch,
    reload: () => loadFolderContents(currentFolderId),
  };
}
