import { FilesTable } from "@/components/tables/files-table";

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

interface DashboardFilesViewProps {
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
      onBulkDelete={onBulkDelete ? (files) => onBulkDelete(files) : undefined}
      onBulkShare={onBulkShare ? (files) => onBulkShare(files) : undefined}
      onBulkDownload={onBulkDownload ? (files) => onBulkDownload(files) : undefined}
      setClearSelectionCallback={setClearSelectionCallback}
    />
  );
}
