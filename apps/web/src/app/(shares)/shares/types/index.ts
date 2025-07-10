export interface SharesSearchProps {
  searchQuery: string;
  onSearchChange: (value: string) => void;
  onCreateShare: () => void;
  totalShares: number;
  filteredCount: number;
}

export interface SharesTableContainerProps {
  shares: any[];
  onCopyLink: (share: any) => void;
  onCreateShare: () => void;
  shareManager: any;
}

export interface SharesModalsProps {
  isCreateModalOpen: boolean;
  onCloseCreateModal: () => void;
  shareToViewDetails: any;
  shareToGenerateLink: any;
  shareManager: any;
  fileManager: any;
  onSuccess: () => void;
  onCloseViewDetails: () => void;
  onCloseGenerateLink: () => void;
  smtpEnabled?: string;
}
