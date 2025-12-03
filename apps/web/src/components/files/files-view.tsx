import { useState } from "react";
import { IconLayoutGrid, IconTable } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { FilesGrid } from "@/components/tables/files-grid";
import { FilesTable } from "@/components/tables/files-table";
import { Button } from "@/components/ui/button";

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

interface FilesViewProps {
  files: File[];
  folders?: Folder[];
  onPreview?: (file: File) => void;
  onRename: (file: File) => void;
  onUpdateName: (fileId: string, newName: string) => void;
  onUpdateDescription: (fileId: string, newDescription: string) => void;
  onDownload: (objectName: string, fileName: string) => void;
  onShare: (file: File) => void;
  onDelete: (file: File) => void;
  onBulkDelete?: (files: File[], folders: Folder[]) => void;
  onBulkShare?: (files: File[], folders: Folder[]) => void;
  onBulkDownload?: (files: File[], folders: Folder[]) => void;
  setClearSelectionCallback?: (callback: () => void) => void;
}

export type ViewMode = "table" | "grid";

export function FilesView({
  files,
  onPreview,
  onRename,
  onUpdateName,
  onUpdateDescription,
  onDownload,
  onShare,
  onDelete,
  onBulkDelete,
  onBulkShare,
  onBulkDownload,
  setClearSelectionCallback,
}: FilesViewProps) {
  const t = useTranslations();
  const [viewMode, setViewMode] = useState<ViewMode>("table");

  const baseProps = {
    files,
    folders: [],
    onPreview,
    onRename,
    onDownload,
    onShare,
    onDelete,
    onBulkDelete: (files: File[], folders: Folder[]) => onBulkDelete?.(files, folders),
    onBulkShare: (files: File[], folders: Folder[]) => onBulkShare?.(files, folders),
    onBulkDownload: (files: File[], folders: Folder[]) => onBulkDownload?.(files, folders),
    setClearSelectionCallback,
  };

  const tableProps = {
    ...baseProps,
    onUpdateName,
    onUpdateDescription,
  };

  const gridProps = baseProps;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-muted-foreground">{t("files.viewMode.label")}:</span>
          <div className="flex items-center border rounded-lg p-1">
            <Button
              variant={viewMode === "table" ? "default" : "ghost"}
              size="sm"
              className="h-8 px-3"
              onClick={() => setViewMode("table")}
            >
              <IconTable className="h-4 w-4" />
              {t("files.viewMode.table")}
            </Button>
            <Button
              variant={viewMode === "grid" ? "default" : "ghost"}
              size="sm"
              className="h-8 px-3"
              onClick={() => setViewMode("grid")}
            >
              <IconLayoutGrid className="h-4 w-4" />
              {t("files.viewMode.grid")}
            </Button>
          </div>
        </div>

        <div className="text-sm text-muted-foreground">{t("files.totalFiles", { count: files.length })}</div>
      </div>

      {viewMode === "table" ? <FilesTable {...tableProps} /> : <FilesGrid {...gridProps} />}
    </div>
  );
}
