import { useEffect, useState } from "react";
import { IconLayoutGrid, IconSearch, IconTable } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { ShareBreadcrumbNavigation } from "@/app/(shares)/s/[alias]/components/browse/share-breadcrumb-navigation";
import { ShareEmptyState } from "@/app/(shares)/s/[alias]/components/browse/share-empty-state";
import { ShareFilesTable } from "@/app/(shares)/s/[alias]/components/browse/share-files-table";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

interface ShareBrowseManagerProps {
  shareId: string;
  folders: any[];
  files: any[];
  path: any[];
  password?: string;
  isLoading: boolean;
  searchQuery: string;
  onSearch: (query: string) => void;
  onNavigateToFolder: (folderId?: string) => void;
  onDownload: (objectName: string, fileName: string) => Promise<void>;
  onDownloadFolder: (folderId: string, folderName: string) => Promise<void>;
  onBulkDownload?: () => Promise<void>;
}

export type ViewMode = "table" | "grid";

const VIEW_MODE_KEY = "share-view-mode";

export function ShareBrowseManager({
  shareId,
  folders,
  files,
  path,
  password,
  isLoading,
  searchQuery,
  onSearch,
  onNavigateToFolder,
  onDownload,
  onDownloadFolder,
  onBulkDownload,
}: ShareBrowseManagerProps) {
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

  const hasContent = folders.length > 0 || files.length > 0;
  const showEmptyState = !hasContent && !searchQuery && !isLoading;

  const filteredFolders = folders.filter((folder) => folder.name.toLowerCase().includes(searchQuery.toLowerCase()));

  const filteredFiles = files.filter((file) => file.name.toLowerCase().includes(searchQuery.toLowerCase()));

  const hasFilteredContent = filteredFolders.length > 0 || filteredFiles.length > 0;

  return (
    <div className="space-y-4">
      {/* Navigation Bar */}
      <div className="flex items-center justify-between">
        <ShareBreadcrumbNavigation path={path} onNavigate={onNavigateToFolder} />

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
              disabled
              title="Grid view coming soon"
            >
              <IconLayoutGrid className="h-4 w-4 opacity-50" />
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
        <ShareEmptyState />
      ) : (
        <div className="space-y-4">
          <ShareFilesTable
            files={filteredFiles}
            folders={filteredFolders}
            onDownload={onDownload}
            onDownloadFolder={onDownloadFolder}
            onNavigateToFolder={onNavigateToFolder}
          />

          {/* No results message */}
          {searchQuery && !hasFilteredContent && (
            <div className="text-center py-8">
              <p className="text-muted-foreground">No results found for "{searchQuery}".</p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
