import { EnhancedFileManagerHook } from "@/hooks/use-enhanced-file-manager";
import { ShareManagerHook } from "@/hooks/use-share-manager";
import { Share } from "@/http/endpoints/shares/types";

export interface RecentFilesProps {
  files: any[];
  fileManager: EnhancedFileManagerHook;
  isUploadModalOpen: boolean;
  onOpenUploadModal: () => void;
}

export interface RecentSharesProps {
  shares: Share[];
  shareManager: ShareManagerHook;
  isCreateModalOpen: boolean;
  onOpenCreateModal: () => void;
  onCopyLink: (share: Share) => void;
}

export interface StorageUsageProps {
  diskSpace: {
    diskSizeGB: number;
    diskUsedGB: number;
    diskAvailableGB: number;
    uploadAllowed: boolean;
  } | null;
  diskSpaceError?: string | null;
  onRetry?: () => void;
}

export interface DashboardModalsProps {
  modals: {
    isUploadModalOpen: boolean;
    isCreateModalOpen: boolean;
    onCloseUploadModal: () => void;
    onCloseCreateModal: () => void;
  };
  fileManager: EnhancedFileManagerHook;
  shareManager: ShareManagerHook;
  onSuccess: () => Promise<void>;
  smtpEnabled?: string;
}
