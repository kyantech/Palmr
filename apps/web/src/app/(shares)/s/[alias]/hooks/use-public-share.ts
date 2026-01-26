"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getShareByAlias } from "@/http/endpoints/index";
import type { Share } from "@/http/endpoints/shares/types";
import { getCachedDownloadUrl } from "@/lib/download-url-cache";

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

        // Find root folders in the shared context
        // Root is either folders with no parent OR folders whose parent is not in the share
        const folders = allFolders.filter((folder: any) => {
          if (folderId === null) {
            // Show folders at the root of the share (no parent or parent not in share)
            return !folder.parentId || !shareFolderIds.has(folder.parentId);
          } else {
            // Show folders that are direct children of the current folder
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

      // Get all files in this folder and subfolders with their paths
      const getFolderFilesWithPath = (
        targetFolderId: string,
        currentPath: string = ""
      ): Array<{ file: any; path: string }> => {
        const filesWithPath: Array<{ file: any; path: string }> = [];

        // Get direct files in this folder
        const directFiles = share.files?.filter((f) => f.folderId === targetFolderId) || [];
        directFiles.forEach((file) => {
          filesWithPath.push({ file, path: currentPath });
        });

        // Get subfolders and process them recursively
        const subfolders = share.folders?.filter((f) => f.parentId === targetFolderId) || [];
        for (const subfolder of subfolders) {
          const subfolderPath = currentPath ? `${currentPath}/${subfolder.name}` : subfolder.name;
          filesWithPath.push(...getFolderFilesWithPath(subfolder.id, subfolderPath));
        }

        return filesWithPath;
      };

      const folderFilesWithPath = getFolderFilesWithPath(folderId);

      if (folderFilesWithPath.length === 0) {
        toast.error(t("shareManager.noFilesToDownload"));
        return;
      }

      const loadingToast = toast.loading(t("shareManager.creatingZip"));

      try {
        // Get presigned URLs for all files with their relative paths
        const downloadItems = await Promise.all(
          folderFilesWithPath.map(async ({ file, path }) => {
            const url = await getCachedDownloadUrl(
              file.objectName,
              password ? { headers: { "x-share-password": password } } : undefined
            );
            return {
              url,
              name: path ? `${path}/${file.name}` : file.name,
            };
          })
        );

        // Create ZIP with all files
        const { downloadFilesAsZip } = await import("@/utils/zip-download");
        const zipName = `${folderName}.zip`;
        await downloadFilesAsZip(downloadItems, zipName);

        toast.dismiss(loadingToast);
        toast.success(t("shareManager.zipDownloadSuccess"));
      } catch (error) {
        toast.dismiss(loadingToast);
        toast.error(t("shareManager.zipDownloadError"));
        throw error;
      }
    } catch (error) {
      console.error("Error downloading folder:", error);
      throw error;
    }
  };

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
      if (objectName.startsWith("folder:")) {
        const folderId = objectName.replace("folder:", "");
        await handleFolderDownload(folderId, fileName);
        return;
      }

      const loadingToast = toast.loading(t("share.messages.downloadStarted"));

      const url = await getCachedDownloadUrl(
        objectName,
        password ? { headers: { "x-share-password": password } } : undefined
      );

      const link = document.createElement("a");
      link.href = url;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);

      toast.dismiss(loadingToast);
      toast.success(t("shareManager.downloadSuccess"));
    } catch (error) {
      console.error("Error downloading file:", error);
      toast.error(t("share.errors.downloadFailed"));
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
      const loadingToast = toast.loading(t("shareManager.creatingZip"));

      try {
        // Helper function to get all files in a folder recursively with paths
        const getFolderFilesWithPath = (
          targetFolderId: string,
          currentPath: string = ""
        ): Array<{ file: any; path: string }> => {
          const filesWithPath: Array<{ file: any; path: string }> = [];

          // Get direct files in this folder
          const directFiles = share.files?.filter((f) => f.folderId === targetFolderId) || [];
          directFiles.forEach((file) => {
            filesWithPath.push({ file, path: currentPath });
          });

          // Get subfolders and process them recursively
          const subfolders = share.folders?.filter((f) => f.parentId === targetFolderId) || [];
          for (const subfolder of subfolders) {
            const subfolderPath = currentPath ? `${currentPath}/${subfolder.name}` : subfolder.name;
            filesWithPath.push(...getFolderFilesWithPath(subfolder.id, subfolderPath));
          }

          return filesWithPath;
        };

        const allFilesToDownload: Array<{ url: string; name: string }> = [];

        // Get presigned URLs for root level files (not in any folder)
        const rootFiles = share.files?.filter((f) => !f.folderId) || [];
        const rootFileItems = await Promise.all(
          rootFiles.map(async (file) => {
            const url = await getCachedDownloadUrl(
              file.objectName,
              password ? { headers: { "x-share-password": password } } : undefined
            );
            return {
              url,
              name: file.name,
            };
          })
        );
        allFilesToDownload.push(...rootFileItems);

        // Get presigned URLs for files in root level folders
        const rootFolders = share.folders?.filter((f) => !f.parentId) || [];
        for (const folder of rootFolders) {
          const folderFilesWithPath = getFolderFilesWithPath(folder.id, folder.name);

          const folderFileItems = await Promise.all(
            folderFilesWithPath.map(async ({ file, path }) => {
              const url = await getCachedDownloadUrl(
                file.objectName,
                password ? { headers: { "x-share-password": password } } : undefined
              );
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
        const zipName = `${share.name || t("shareManager.defaultShareName")}.zip`;
        await downloadFilesAsZip(allFilesToDownload, zipName);

        toast.dismiss(loadingToast);
        toast.success(t("shareManager.zipDownloadSuccess"));
      } catch (error) {
        toast.dismiss(loadingToast);
        toast.error(t("shareManager.zipDownloadError"));
        throw error;
      }
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
      const loadingToast = toast.loading(t("shareManager.creatingZip"));

      try {
        // Helper function to get all files in a folder recursively with paths
        const getFolderFilesWithPath = (
          targetFolderId: string,
          currentPath: string = ""
        ): Array<{ file: any; path: string }> => {
          const filesWithPath: Array<{ file: any; path: string }> = [];

          // Get direct files in this folder
          const directFiles = share.files?.filter((f) => f.folderId === targetFolderId) || [];
          directFiles.forEach((file) => {
            filesWithPath.push({ file, path: currentPath });
          });

          // Get subfolders and process them recursively
          const subfolders = share.folders?.filter((f) => f.parentId === targetFolderId) || [];
          for (const subfolder of subfolders) {
            const subfolderPath = currentPath ? `${currentPath}/${subfolder.name}` : subfolder.name;
            filesWithPath.push(...getFolderFilesWithPath(subfolder.id, subfolderPath));
          }

          return filesWithPath;
        };

        const allFilesToDownload: Array<{ url: string; name: string }> = [];

        // Get presigned URLs for direct files (not in folders)
        const directFileItems = await Promise.all(
          files.map(async (file) => {
            const url = await getCachedDownloadUrl(
              file.objectName,
              password ? { headers: { "x-share-password": password } } : undefined
            );
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
              const url = await getCachedDownloadUrl(
                file.objectName,
                password ? { headers: { "x-share-password": password } } : undefined
              );
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
        const finalZipName = `${share.name || t("shareManager.defaultShareName")}-selected.zip`;
        await downloadFilesAsZip(allFilesToDownload, finalZipName);

        toast.dismiss(loadingToast);
        toast.success(t("shareManager.zipDownloadSuccess"));
      } catch (error) {
        toast.dismiss(loadingToast);
        toast.error(t("shareManager.zipDownloadError"));
        throw error;
      }
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [share, urlFolderSlug]);

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
