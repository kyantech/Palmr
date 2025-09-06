import { FilesTable } from "@/components/tables/files-table";

interface File {
  id: string;
  name: string;
  description?: string;
  extension: string;
  size: number;
  objectName: string;
  userId: string;
  folderId?: string | null;
  createdAt: string;
  updatedAt: string;
}

interface DashboardFilesViewProps {
  files: File[];
  onPreview: (file: File) => void;
  onRename: (file: File) => void;
  onUpdateName: (fileId: string, newName: string) => void;
  onUpdateDescription: (fileId: string, newDescription: string) => void;
  onDownload: (objectName: string, fileName: string) => void;
  onShare: (file: File) => void;
  onDelete: (file: File) => void;
  onBulkDelete?: (files: File[], folders: any[]) => void;
  onBulkShare?: (files: File[], folders: any[]) => void;
  onBulkDownload?: (files: File[], folders: any[]) => void;
  setClearSelectionCallback?: (callback: () => void) => void;
}

export function DashboardFilesView({
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
}: DashboardFilesViewProps) {
  return (
    <FilesTable
      files={files}
      folders={[]}
      onPreview={onPreview}
      onRename={onRename}
      onUpdateName={onUpdateName}
      onUpdateDescription={onUpdateDescription}
      onDownload={onDownload}
      onShare={onShare}
      onDelete={onDelete}
      onBulkDelete={(files, folders) => onBulkDelete?.(files, folders)}
      onBulkShare={(files, folders) => onBulkShare?.(files, folders)}
      onBulkDownload={(files, folders) => onBulkDownload?.(files, folders)}
      setClearSelectionCallback={setClearSelectionCallback}
    />
  );
}
