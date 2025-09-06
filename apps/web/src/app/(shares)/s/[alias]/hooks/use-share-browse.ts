"use client";

import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { getShare } from "@/http/endpoints/shares";

interface ShareBrowseState {
  folders: any[];
  files: any[];
  path: any[];
  isLoading: boolean;
  error: string | null;
}

interface UseShareBrowseProps {
  shareId: string;
  password?: string;
}

export function useShareBrowse({ shareId, password }: UseShareBrowseProps) {
  const [state, setState] = useState<ShareBrowseState>({
    folders: [],
    files: [],
    path: [],
    isLoading: true,
    error: null,
  });

  const [currentFolderId, setCurrentFolderId] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");

  const loadFolderContents = useCallback(
    async (folderId: string | null) => {
      try {
        setState((prev) => ({ ...prev, isLoading: true, error: null }));

        // Get the full share data
        const response = await getShare(shareId, password ? { password } : undefined);
        const shareData = response.data.share || response.data;

        console.log("Share browse - Raw share data:", shareData);
        console.log("Share browse - All files:", shareData.files);
        console.log("Share browse - All folders:", shareData.folders);

        // Build hierarchical structure client-side
        const allFiles = shareData.files || [];
        const allFolders = shareData.folders || [];

        console.log("Share browse - Processing for folderId:", folderId);

        // Filter for current folder level
        console.log("Share browse - About to filter folders for folderId:", folderId);

        // Create a set of all folder IDs that are in the share for quick lookup
        const shareFolderIds = new Set(allFolders.map((f) => f.id));

        allFolders.forEach((folder: any, index) => {
          const hasParentInShare = folder.parentId && shareFolderIds.has(folder.parentId);
          console.log(`Folder ${index}:`, {
            id: folder.id,
            name: folder.name,
            parentId: folder.parentId,
            parentIdNormalized: folder.parentId || null,
            hasParentInShare,
            shouldShowAtRoot: !hasParentInShare && folderId === null,
            matchesTarget: (folder.parentId || null) === folderId,
          });
        });

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

        console.log("Share browse - Filtered folders:", folders);
        console.log("Share browse - Filtered files:", files);

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

        setState({
          folders,
          files,
          path,
          isLoading: false,
          error: null,
        });
      } catch (error: any) {
        console.error("Error loading folder contents:", error);

        let errorMessage = "Failed to load folder contents";

        if (error.response?.status === 401) {
          errorMessage = "Password required or invalid";
        } else if (error.response?.status === 404) {
          errorMessage = "Share or folder not found";
        } else if (error.response?.status === 403) {
          errorMessage = "Share has reached maximum views";
        } else if (error.response?.status === 410) {
          errorMessage = "Share has expired";
        }

        setState((prev) => ({
          ...prev,
          isLoading: false,
          error: errorMessage,
        }));

        toast.error(errorMessage);
      }
    },
    [shareId, password]
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

  // Filter content based on search query
  const filteredFolders = state.folders.filter((folder) =>
    folder.name?.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const filteredFiles = state.files.filter((file) => file.name?.toLowerCase().includes(searchQuery.toLowerCase()));

  // Load initial content
  useEffect(() => {
    loadFolderContents(null);
  }, [loadFolderContents]);

  return {
    // State
    folders: filteredFolders,
    files: filteredFiles,
    path: state.path,
    isLoading: state.isLoading,
    error: state.error,
    currentFolderId,
    searchQuery,

    // Actions
    navigateToFolder,
    handleSearch,
    reload: () => loadFolderContents(currentFolderId),
  };
}
