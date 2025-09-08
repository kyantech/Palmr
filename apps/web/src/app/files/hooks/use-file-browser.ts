"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { useEnhancedFileManager } from "@/hooks/use-enhanced-file-manager";
import { listFiles } from "@/http/endpoints";
import { listFolders } from "@/http/endpoints/folders";

export function useFileBrowser() {
  const t = useTranslations();
  const router = useRouter();
  const searchParams = useSearchParams();

  const [files, setFiles] = useState<any[]>([]);
  const [folders, setFolders] = useState<any[]>([]);
  const [allFiles, setAllFiles] = useState<any[]>([]);
  const [allFolders, setAllFolders] = useState<any[]>([]);
  const [currentPath, setCurrentPath] = useState<any[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState("");
  const [isUploadModalOpen, setIsUploadModalOpen] = useState(false);
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | undefined>();
  const [dataLoaded, setDataLoaded] = useState(false);
  const isNavigatingRef = useRef(false);

  const urlFolderId = searchParams.get("folder") || null;
  const currentFolderId = urlFolderId;

  const setClearSelectionCallback = useCallback((callback: () => void) => {
    setClearSelectionCallbackState(() => callback);
  }, []);

  const buildBreadcrumbPath = useCallback((allFolders: any[], folderId: string): any[] => {
    const path: any[] = [];
    let currentId: string | null = folderId;

    while (currentId) {
      const folder = allFolders.find((f) => f.id === currentId);
      if (folder) {
        path.unshift(folder);
        currentId = folder.parentId;
      } else {
        break;
      }
    }

    return path;
  }, []);

  const buildFolderPath = useCallback((allFolders: any[], folderId: string | null): string => {
    if (!folderId) return "";

    const pathParts: string[] = [];
    let currentId: string | null = folderId;

    while (currentId) {
      const folder = allFolders.find((f) => f.id === currentId);
      if (folder) {
        pathParts.unshift(folder.name);
        currentId = folder.parentId;
      } else {
        break;
      }
    }

    return pathParts.join(" / ");
  }, []);

  const navigateToFolderDirect = useCallback(
    (targetFolderId: string | null) => {
      // Filter existing data for instant navigation
      const currentFiles = allFiles.filter((file: any) => (file.folderId || null) === targetFolderId);
      const currentFolders = allFolders.filter((folder: any) => (folder.parentId || null) === targetFolderId);

      const sortedFiles = [...currentFiles].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      const sortedFolders = [...currentFolders].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      setFiles(sortedFiles);
      setFolders(sortedFolders);

      if (targetFolderId) {
        const path = buildBreadcrumbPath(allFolders, targetFolderId);
        setCurrentPath(path);
      } else {
        setCurrentPath([]);
      }

      // Update URL without triggering reload
      const params = new URLSearchParams(searchParams);
      if (targetFolderId) {
        params.set("folder", targetFolderId);
      } else {
        params.delete("folder");
      }
      window.history.pushState({}, "", `/files?${params.toString()}`);
    },
    [allFiles, allFolders, buildBreadcrumbPath, searchParams]
  );

  const navigateToFolder = useCallback(
    (folderId?: string) => {
      const targetFolderId = folderId || null;

      if (dataLoaded && allFiles.length > 0) {
        // Use direct navigation
        isNavigatingRef.current = true;
        navigateToFolderDirect(targetFolderId);
        // Reset flag after navigation
        setTimeout(() => {
          isNavigatingRef.current = false;
        }, 0);
      } else {
        // Fallback to traditional navigation for initial load
        const params = new URLSearchParams(searchParams);
        if (folderId) {
          params.set("folder", folderId);
        } else {
          params.delete("folder");
        }
        router.push(`/files?${params.toString()}`);
      }
    },
    [dataLoaded, allFiles.length, navigateToFolderDirect, searchParams, router]
  );

  const navigateToRoot = useCallback(() => {
    navigateToFolder();
  }, [navigateToFolder]);

  const loadFiles = useCallback(async () => {
    try {
      setIsLoading(true);

      const [filesResponse, foldersResponse] = await Promise.all([listFiles(), listFolders()]);

      const fetchedFiles = filesResponse.data.files || [];
      const fetchedFolders = foldersResponse.data.folders || [];

      setAllFiles(fetchedFiles);
      setAllFolders(fetchedFolders);
      setDataLoaded(true);

      const currentFiles = fetchedFiles.filter((file: any) => (file.folderId || null) === urlFolderId);
      const currentFolders = fetchedFolders.filter((folder: any) => (folder.parentId || null) === urlFolderId);

      const sortedFiles = [...currentFiles].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      const sortedFolders = [...currentFolders].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      setFiles(sortedFiles);
      setFolders(sortedFolders);

      if (urlFolderId) {
        const path = buildBreadcrumbPath(fetchedFolders, urlFolderId);
        setCurrentPath(path);
      } else {
        setCurrentPath([]);
      }
    } catch {
      toast.error(t("files.loadError"));
    } finally {
      setIsLoading(false);
    }
  }, [urlFolderId, buildBreadcrumbPath, t]);

  const fileManager = useEnhancedFileManager(loadFiles, clearSelectionCallback);

  const getImmediateChildFoldersWithMatches = useCallback(() => {
    if (!searchQuery) return [];

    const matchingItems = new Set<string>();

    allFiles
      .filter((file: any) => file.name.toLowerCase().includes(searchQuery.toLowerCase()))
      .forEach((file: any) => {
        if (file.folderId) {
          let currentId = file.folderId;
          while (currentId) {
            const folder = allFolders.find((f: any) => f.id === currentId);
            if (folder) {
              if ((folder.parentId || null) === currentFolderId) {
                matchingItems.add(folder.id);
                break;
              }
              currentId = folder.parentId;
            } else {
              break;
            }
          }
        }
      });

    allFolders
      .filter((folder: any) => folder.name.toLowerCase().includes(searchQuery.toLowerCase()))
      .forEach((folder: any) => {
        let currentId = folder.id;
        while (currentId) {
          const folderInPath = allFolders.find((f: any) => f.id === currentId);
          if (folderInPath) {
            if ((folderInPath.parentId || null) === currentFolderId) {
              matchingItems.add(folderInPath.id);
              break;
            }
            currentId = folderInPath.parentId;
          } else {
            break;
          }
        }
      });

    return allFolders
      .filter((folder: any) => matchingItems.has(folder.id))
      .sort((a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime());
  }, [searchQuery, allFiles, allFolders, currentFolderId]);

  const filteredFiles = searchQuery
    ? allFiles
        .filter(
          (file: any) =>
            file.name.toLowerCase().includes(searchQuery.toLowerCase()) && (file.folderId || null) === currentFolderId
        )
        .sort((a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime())
    : files;

  const filteredFolders = searchQuery ? getImmediateChildFoldersWithMatches() : folders;

  useEffect(() => {
    if (!isNavigatingRef.current) {
      loadFiles();
    }
  }, [loadFiles]);

  return {
    isLoading,
    files,
    folders,
    currentPath,
    currentFolderId,
    searchQuery,

    navigateToFolder,
    navigateToRoot,

    modals: {
      isUploadModalOpen,
      onOpenUploadModal: () => setIsUploadModalOpen(true),
      onCloseUploadModal: () => setIsUploadModalOpen(false),
    },

    fileManager: {
      ...fileManager,
      setClearSelectionCallback,
    } as typeof fileManager & { setClearSelectionCallback: typeof setClearSelectionCallback },

    filteredFiles,
    filteredFolders,

    handleSearch: setSearchQuery,
    loadFiles,

    allFiles,
    allFolders,
    buildFolderPath,
  };
}

export const useFiles = useFileBrowser;
