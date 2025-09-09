import { EnhancedFileManagerHook } from "@/hooks/use-enhanced-file-manager";

export interface HeaderProps {
  onUpload: () => void;
  onCreateFolder?: () => void;
}

export interface FileListProps {
  files: any[];
  filteredFiles: any[];
  folders?: any[];
  filteredFolders?: any[];
  fileManager: EnhancedFileManagerHook;
  searchQuery: string;
  onSearch: (query: string) => void;
  onUpload: () => void;
}

export interface SearchBarProps {
  searchQuery: string;
  onSearch: (query: string) => void;
  totalFiles: number;
  totalFolders?: number;
  filteredCount: number;
  filteredFolders?: number;
}

export interface FilesModalsProps {
  fileManager: EnhancedFileManagerHook;
  modals: {
    isUploadModalOpen: boolean;
    onCloseUploadModal: () => void;
  };
  onSuccess: () => Promise<void>;
}
