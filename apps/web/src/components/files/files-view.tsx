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
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface FilesViewProps {
  files: File[];
  onPreview: (file: File) => void;
  onRename: (file: File) => void;
  onUpdateName: (fileId: string, newName: string) => void;
  onUpdateDescription: (fileId: string, newDescription: string) => void;
  onDownload: (objectName: string, fileName: string) => void;
  onShare: (file: File) => void;
  onDelete: (file: File) => void;
  onBulkDelete?: (files: File[]) => void;
  onBulkShare?: (files: File[]) => void;
  onBulkDownload?: (files: File[]) => void;
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

  const commonProps = {
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
  };

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

      {viewMode === "table" ? <FilesTable {...commonProps} /> : <FilesGrid {...commonProps} />}
    </div>
  );
}
