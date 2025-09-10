"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getShareByAlias } from "@/http/endpoints/index";
import type { Share } from "@/http/endpoints/shares/types";
import {
  bulkDownloadShareWithQueue,
  downloadFileWithQueue,
  downloadShareFolderWithQueue,
} from "@/utils/download-queue-utils";

const createSlug = (name: string): string => {
  return name
    .toLowerCase()
    .trim()
    .replace(/[^\w\s-]/g, "")
    .replace(/[\s_-]+/g, "-")
    .replace(/^-+|-+$/g, "");
};

const createFolderPathSlug = (allFolders: any[], folderId: string): string => {
  const path: string[] = [];
  let currentId: string | null = folderId;

  while (currentId) {
    const folder = allFolders.find((f) => f.id === currentId);
    if (folder) {
      const slug = createSlug(folder.name);
      path.unshift(slug || folder.id);
      currentId = folder.parentId;
    } else {
      break;
    }
  }

  return path.join("/");
};

const findFolderByPathSlug = (folders: any[], pathSlug: string): any | null => {
  const pathParts = pathSlug.split("/");
  let currentFolders = folders.filter((f) => !f.parentId);
  let currentFolder: any = null;

  for (const slugPart of pathParts) {
    currentFolder = currentFolders.find((folder) => {
      const slug = createSlug(folder.name);
      return slug === slugPart || folder.id === slugPart;
    });

    if (!currentFolder) return null;
    currentFolders = folders.filter((f) => f.parentId === currentFolder.id);
  }

  return currentFolder;
};

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
  const searchParams = useSearchParams();
  const router = useRouter();
  const alias = params?.alias as string;
  const [share, setShare] = useState<Share | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [password, setPassword] = useState("");
  const [isPasswordModalOpen, setIsPasswordModalOpen] = useState(false);
  const [isPasswordError, setIsPasswordError] = useState(false);

  const [browseState, setBrowseState] = useState<ShareBrowseState>({
    folders: [],
    files: [],
    path: [],
    isLoading: true,
    error: null,
  });
  const urlFolderSlug = searchParams.get("folder") || null;
  const [currentFolderId, setCurrentFolderId] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");

  const getFolderIdFromPathSlug = useCallback((pathSlug: string | null, folders: any[]): string | null => {
    if (!pathSlug) return null;
    const folder = findFolderByPathSlug(folders, pathSlug);
    return folder ? folder.id : null;
  }, []);

  const getFolderPathSlugFromId = useCallback((folderId: string | null, folders: any[]): string | null => {
    if (!folderId) return null;
    return createFolderPathSlug(folders, folderId);
  }, []);

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
      } catch (error: any) {
        handleShareError(error);
      } finally {
        setIsLoading(false);
      }
    },
    [alias, t]
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

        const allFiles = share.files || [];
        const allFolders = share.folders || [];

        const shareFolderIds = new Set(allFolders.map((f) => f.id));

        const folders = allFolders.filter((folder: any) => {
          if (folderId === null) {
            return !folder.parentId || !shareFolderIds.has(folder.parentId);
          } else {
            return folder.parentId === folderId;
          }
        });
        const files = allFiles.filter((file: any) => (file.folderId || null) === folderId);

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

      const params = new URLSearchParams(searchParams);
      if (targetFolderId && share?.folders) {
        const folderPathSlug = getFolderPathSlugFromId(targetFolderId, share.folders);
        if (folderPathSlug) {
          params.set("folder", folderPathSlug);
        } else {
          params.delete("folder");
        }
      } else {
        params.delete("folder");
      }
      router.push(`/s/${alias}?${params.toString()}`);
    },
    [loadFolderContents, searchParams, router, alias, share?.folders, getFolderPathSlugFromId]
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

      await downloadShareFolderWithQueue(folderId, folderName, share.files || [], share.folders || [], {
        silent: true,
        showToasts: false,
        sharePassword: password,
      });
    } catch (error) {
      console.error("Error downloading folder:", error);
      throw error;
    }
  };

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
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
            sharePassword: password,
          }),
          {
            loading: t("share.messages.downloadStarted"),
            success: t("shareManager.downloadSuccess"),
            error: t("share.errors.downloadFailed"),
          }
        );
      }
    } catch {}
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

      if (share.files) {
        share.files.forEach((file) => {
          if (!file.folderId) {
            allItems.push({
              objectName: file.objectName,
              name: file.name,
              type: "file",
            });
          }
        });
      }

      if (share.folders) {
        const folderIds = new Set(share.folders.map((f) => f.id));
        share.folders.forEach((folder) => {
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
        bulkDownloadShareWithQueue(
          allItems,
          share.files || [],
          share.folders || [],
          zipName,
          undefined,
          true,
          share.id,
          password
        ).then(() => {}),
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
        bulkDownloadShareWithQueue(
          allItems,
          share.files || [],
          share.folders || [],
          zipName,
          undefined,
          false,
          password
        ).then(() => {}),
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [alias]);

  useEffect(() => {
    if (share) {
      const resolvedFolderId = getFolderIdFromPathSlug(urlFolderSlug, share.folders || []);
      setCurrentFolderId(resolvedFolderId);
      loadFolderContents(resolvedFolderId);
    }
  }, [share, loadFolderContents, urlFolderSlug, getFolderIdFromPathSlug]);

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
