"use client";

import { useCallback, useEffect, useState } from "react";
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

  // State following original pattern with any[] type
  const [files, setFiles] = useState<any[]>([]);
  const [folders, setFolders] = useState<any[]>([]);
  const [currentPath, setCurrentPath] = useState<any[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState("");
  const [isUploadModalOpen, setIsUploadModalOpen] = useState(false);
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | undefined>();

  const currentFolderId = searchParams.get("folder") || null;

  const setClearSelectionCallback = useCallback((callback: () => void) => {
    setClearSelectionCallbackState(() => callback);
  }, []);

  // Navigation actions following original pattern
  const navigateToFolder = useCallback(
    (folderId?: string) => {
      const params = new URLSearchParams(searchParams);
      if (folderId) {
        params.set("folder", folderId);
      } else {
        params.delete("folder");
      }
      router.push(`/files?${params.toString()}`);
    },
    [router, searchParams]
  );

  const navigateToRoot = useCallback(() => {
    const params = new URLSearchParams(searchParams);
    params.delete("folder");
    router.push(`/files?${params.toString()}`);
  }, [router, searchParams]);

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

  const loadFiles = useCallback(async () => {
    try {
      setIsLoading(true);

      // Load files and folders in parallel following original async pattern
      const [filesResponse, foldersResponse] = await Promise.all([listFiles(), listFolders()]);

      const allFiles = filesResponse.data.files || [];
      const allFolders = foldersResponse.data.folders || [];

      // Filter by current location
      const currentFiles = allFiles.filter((file: any) => (file.folderId || null) === currentFolderId);
      const currentFolders = allFolders.filter((folder: any) => (folder.parentId || null) === currentFolderId);

      // Sort by creation date (newest first) following original pattern
      const sortedFiles = [...currentFiles].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      const sortedFolders = [...currentFolders].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      setFiles(sortedFiles);
      setFolders(sortedFolders);

      // Build breadcrumb path
      if (currentFolderId) {
        const path = buildBreadcrumbPath(allFolders, currentFolderId);
        setCurrentPath(path);
      } else {
        setCurrentPath([]);
      }
    } catch {
      toast.error(t("files.loadError"));
    } finally {
      setIsLoading(false);
    }
  }, [currentFolderId, buildBreadcrumbPath, t]);

  // File manager integration following original pattern
  const fileManager = useEnhancedFileManager(loadFiles, clearSelectionCallback);

  // Filtered results following original pattern
  const filteredFiles = files.filter((file) => file.name.toLowerCase().includes(searchQuery.toLowerCase()));
  const filteredFolders = folders.filter((folder) => folder.name.toLowerCase().includes(searchQuery.toLowerCase()));

  useEffect(() => {
    loadFiles();
  }, [loadFiles]);

  return {
    // State
    isLoading,
    files,
    folders,
    currentPath,
    currentFolderId,
    searchQuery,

    // Navigation
    navigateToFolder,
    navigateToRoot,

    // Modals following original pattern
    modals: {
      isUploadModalOpen,
      onOpenUploadModal: () => setIsUploadModalOpen(true),
      onCloseUploadModal: () => setIsUploadModalOpen(false),
    },

    // File manager integration
    fileManager: {
      ...fileManager,
      setClearSelectionCallback,
    } as typeof fileManager & { setClearSelectionCallback: typeof setClearSelectionCallback },

    // Filtered results
    filteredFiles,
    filteredFolders,

    // Actions
    handleSearch: setSearchQuery,
    loadFiles,
  };
}

// Export alias for backward compatibility
export const useFiles = useFileBrowser;
