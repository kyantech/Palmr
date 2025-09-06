import { Share } from "@/http/endpoints/shares/types";

export interface ShareFile {
  id: string;
  name: string;
  size: string;
  objectName: string;
  createdAt: string;
}

export interface ShareFolder {
  id: string;
  name: string;
  totalSize: string | null;
  createdAt: string;
}

export interface ShareFilesTableProps {
  files: ShareFile[];
  onDownload: (objectName: string, fileName: string) => Promise<void>;
}

export interface ShareContentTableProps {
  files?: ShareFile[];
  folders?: ShareFolder[];
  onDownload: (objectName: string, fileName: string) => Promise<void>;
}

export interface PasswordModalProps {
  isOpen: boolean;
  password: string;
  isError: boolean;
  onPasswordChange: (value: string) => void;
  onSubmit: () => Promise<void>;
}

export interface ShareDetailsProps {
  share: Share;
  password?: string;
  onDownload: (objectName: string, fileName: string) => Promise<void>;
  onBulkDownload?: () => Promise<void>;
}
