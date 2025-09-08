import { EnhancedFileManagerHook } from "@/hooks/use-enhanced-file-manager";

export interface HeaderProps {
  onUpload: () => void;
  onCreateFolder?: () => void;
}

export interface FileListProps {
  files: any[];
  filteredFiles: any[];
  fileManager: EnhancedFileManagerHook;
  searchQuery: string;
  onSearch: (query: string) => void;
  onUpload: () => void;
}

export interface SearchBarProps {
  searchQuery: string;
  onSearch: (query: string) => void;
  totalFiles: number;
  filteredCount: number;
}

export interface FilesModalsProps {
  fileManager: EnhancedFileManagerHook;
  modals: {
    isUploadModalOpen: boolean;
    onCloseUploadModal: () => void;
  };
  onSuccess: () => Promise<void>;
}
