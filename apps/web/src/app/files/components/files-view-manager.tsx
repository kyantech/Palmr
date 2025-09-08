import { useEffect, useState } from "react";
import { IconLayoutGrid, IconSearch, IconTable } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { FilesGrid } from "@/components/tables/files-grid";
import { FilesTable } from "@/components/tables/files-table";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

interface File {
  id: string;
  name: string;
  description?: string;
  extension: string;
  size: number;
  objectName: string;
  userId: string;
  folderId?: string;
  createdAt: string;
  updatedAt: string;
}

interface Folder {
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

interface FilesViewManagerProps {
  files: File[];
  folders?: Folder[];
  searchQuery: string;
  onSearch: (query: string) => void;
  onNavigateToFolder?: (folderId?: string) => void;
  onDownload: (objectName: string, fileName: string) => void;
  breadcrumbs?: React.ReactNode;
  isLoading?: boolean;
  emptyStateComponent?: React.ComponentType;
  isShareMode?: boolean;
  onDeleteFolder?: (folder: Folder) => void;
  onRenameFolder?: (folder: Folder) => void;
  onMoveFolder?: (folder: Folder) => void;
  onMoveFile?: (file: File) => void;
  onShareFolder?: (folder: Folder) => void;
  onDownloadFolder?: (folderId: string, folderName: string) => Promise<void>;
  onPreview?: (file: File) => void;
  onRename?: (file: File) => void;
  onUpdateName?: (fileId: string, newName: string) => void;
  onUpdateDescription?: (fileId: string, newDescription: string) => void;
  onShare?: (file: File) => void;
  onDelete?: (file: File) => void;
  onBulkDelete?: (files: File[], folders: Folder[]) => void;
  onBulkShare?: (files: File[], folders: Folder[]) => void;
  onBulkDownload?: (files: File[], folders: Folder[]) => void;
  onBulkMove?: (files: File[], folders: Folder[]) => void;
  setClearSelectionCallback?: (callback: () => void) => void;
  onUpdateFolderName?: (folderId: string, newName: string) => void;
  onUpdateFolderDescription?: (folderId: string, newDescription: string) => void;
}

export type ViewMode = "table" | "grid";

const VIEW_MODE_KEY = "files-view-mode";

export function FilesViewManager({
  files,
  folders,
  searchQuery,
  onSearch,
  onNavigateToFolder,
  onDownload,
  breadcrumbs,
  isLoading = false,
  emptyStateComponent: EmptyStateComponent,
  isShareMode = false,
  onDeleteFolder,
  onRenameFolder,
  onMoveFolder,
  onMoveFile,
  onShareFolder,
  onDownloadFolder,
  onPreview,
  onRename,
  onUpdateName,
  onUpdateDescription,
  onShare,
  onDelete,
  onBulkDelete,
  onBulkShare,
  onBulkDownload,
  onBulkMove,
  setClearSelectionCallback,
  onUpdateFolderName,
  onUpdateFolderDescription,
}: FilesViewManagerProps) {
  const t = useTranslations();
  const [viewMode, setViewMode] = useState<ViewMode>(() => {
    if (typeof window !== "undefined") {
      return (localStorage.getItem(VIEW_MODE_KEY) as ViewMode) || "table";
    }
    return "table";
  });

  useEffect(() => {
    localStorage.setItem(VIEW_MODE_KEY, viewMode);
  }, [viewMode]);

  const hasContent = (folders?.length || 0) > 0 || files.length > 0;
  const showEmptyState = !hasContent && !searchQuery && !isLoading;

  const isFilesMode = !isShareMode && !!(onDeleteFolder || onRenameFolder || onShare || onDelete);

  const baseProps = {
    files,
    folders: folders || [],
    onNavigateToFolder,
    onDeleteFolder: isShareMode ? undefined : onDeleteFolder,
    onRenameFolder: isShareMode ? undefined : onRenameFolder,
    onMoveFolder: isShareMode ? undefined : onMoveFolder,
    onMoveFile: isShareMode ? undefined : onMoveFile,
    onShareFolder: isShareMode ? undefined : onShareFolder,
    onDownloadFolder,
    onPreview,
    onRename: isShareMode ? undefined : onRename,
    onDownload,
    onShare: isShareMode ? undefined : onShare,
    onDelete: isShareMode ? undefined : onDelete,
    onBulkDelete: isShareMode ? undefined : onBulkDelete,
    onBulkShare: isShareMode ? undefined : onBulkShare,
    onBulkDownload,
    onBulkMove: isShareMode ? undefined : onBulkMove,
    setClearSelectionCallback,
    onUpdateFolderName: isShareMode ? undefined : onUpdateFolderName,
    onUpdateFolderDescription: isShareMode ? undefined : onUpdateFolderDescription,
    showBulkActions: isFilesMode || (isShareMode && !!onBulkDownload),
    isShareMode,
  };

  const tableProps = {
    ...baseProps,
    onUpdateName: isShareMode ? undefined : onUpdateName,
    onUpdateDescription: isShareMode ? undefined : onUpdateDescription,
  };

  const gridProps = baseProps;

  return (
    <div className="space-y-4">
      {/* Breadcrumbs, Search and View Controls */}
      <div className="flex items-center justify-between">
        <div className="flex-1 min-w-0">{breadcrumbs}</div>

        <div className="flex items-center gap-4">
          <div className="relative">
            <IconSearch className="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              type="search"
              placeholder={t("searchBar.placeholder")}
              value={searchQuery}
              onChange={(e) => onSearch(e.target.value)}
              className="max-w-sm pl-10"
            />
          </div>

          <div className="flex items-center border rounded-lg p-1">
            <Button
              variant={viewMode === "table" ? "default" : "ghost"}
              size="sm"
              className="h-8 px-3"
              onClick={() => setViewMode("table")}
            >
              <IconTable className="h-4 w-4" />
            </Button>
            <Button
              variant={viewMode === "grid" ? "default" : "ghost"}
              size="sm"
              className="h-8 px-3"
              onClick={() => setViewMode("grid")}
            >
              <IconLayoutGrid className="h-4 w-4" />
            </Button>
          </div>
        </div>
      </div>

      {isLoading ? (
        <div className="text-center py-8">
          <div className="animate-spin h-8 w-8 border-2 border-current border-t-transparent rounded-full mx-auto mb-4" />
          <p className="text-muted-foreground">Loading...</p>
        </div>
      ) : showEmptyState ? (
        EmptyStateComponent ? (
          <EmptyStateComponent />
        ) : (
          <div className="text-center py-6 flex flex-col items-center gap-2">
            <p className="text-muted-foreground">{t("files.empty.title")}</p>
          </div>
        )
      ) : (
        <div className="space-y-4">
          {viewMode === "table" ? <FilesTable {...tableProps} /> : <FilesGrid {...gridProps} />}

          {/* No results message */}
          {searchQuery && !hasContent && (
            <div className="text-center py-8">
              <p className="text-muted-foreground">{t("searchBar.noResults", { query: searchQuery })}</p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
