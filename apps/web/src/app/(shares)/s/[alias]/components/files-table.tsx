import { useState } from "react";
import { IconDownload, IconEye, IconFolder, IconFolderOpen } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { getFileIcon } from "@/utils/file-icons";
import { formatFileSize } from "@/utils/format-file-size";

interface ShareFile {
  id: string;
  name: string;
  size: string | number;
  objectName: string;
  createdAt: string;
}

interface ShareFolder {
  id: string;
  name: string;
  totalSize?: string | number | null;
  createdAt: string;
}

interface ShareFilesTableProps {
  files?: ShareFile[];
  folders?: ShareFolder[];
  onDownload: (objectName: string, fileName: string) => Promise<void>;
  onDownloadFolder?: (folderId: string, folderName: string) => Promise<void>;
  onNavigateToFolder?: (folderId: string) => void;
  enableNavigation?: boolean;
  sharePassword?: string;
}

export function ShareFilesTable({
  files = [],
  folders = [],
  onDownload,
  onDownloadFolder,
  onNavigateToFolder,
  enableNavigation = false,
  sharePassword,
}: ShareFilesTableProps) {
  const t = useTranslations();
  const [isPreviewOpen, setIsPreviewOpen] = useState(false);
  const [selectedFile, setSelectedFile] = useState<{ name: string; objectName: string; type?: string } | null>(null);

  const formatDateTime = (dateString: string) => {
    const date = new Date(dateString);
    return new Intl.DateTimeFormat("en-US", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    }).format(date);
  };

  const handlePreview = (file: { name: string; objectName: string }) => {
    setSelectedFile(file);
    setIsPreviewOpen(true);
  };

  const handleClosePreview = () => {
    setIsPreviewOpen(false);
    setSelectedFile(null);
  };

  const handleFolderClick = (folderId: string) => {
    if (enableNavigation && onNavigateToFolder) {
      onNavigateToFolder(folderId);
    }
  };

  const handleFolderDownload = async (folderId: string, folderName: string) => {
    if (enableNavigation && onDownloadFolder) {
      await onDownloadFolder(folderId, folderName);
    } else {
      await onDownload(`folder:${folderId}`, folderName);
    }
  };

  const allItems = [
    ...folders.map((folder) => ({ ...folder, type: "folder" as const })),
    ...files.map((file) => ({ ...file, type: "file" as const })),
  ];

  return (
    <div className="flex flex-col gap-4">
      <div className="rounded-lg border shadow-sm overflow-hidden">
        <Table>
          <TableHeader>
            <TableRow className="border-b-0">
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.name")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.size")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.createdAt")}
              </TableHead>
              <TableHead className="h-10 w-[110px] text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.actions")}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {allItems.length === 0 ? (
              <TableRow>
                <TableCell colSpan={4} className="h-32 text-center text-muted-foreground">
                  <div className="flex flex-col items-center gap-2">
                    <IconFolderOpen className="h-16 w-16 text-muted-foreground/50" />
                    <p className="font-medium">
                      {enableNavigation ? "No files or folders" : "No files or folders shared"}
                    </p>
                    <p className="text-sm">{enableNavigation ? "This location is empty" : "This share is empty"}</p>
                  </div>
                </TableCell>
              </TableRow>
            ) : (
              allItems.map((item) => {
                if (item.type === "folder") {
                  return (
                    <TableRow key={`folder-${item.id}`} className="hover:bg-muted/50 transition-colors border-0">
                      <TableCell className="h-12 px-4 border-0">
                        <div className="flex items-center gap-2">
                          <IconFolder className="h-5 w-5 text-blue-600" />
                          {enableNavigation ? (
                            <button
                              className="truncate max-w-[250px] font-medium text-left hover:underline"
                              onClick={() => handleFolderClick(item.id)}
                            >
                              {item.name}
                            </button>
                          ) : (
                            <span className="truncate max-w-[250px] font-medium">{item.name}</span>
                          )}
                        </div>
                      </TableCell>
                      <TableCell className="h-12 px-4">
                        {item.totalSize ? formatFileSize(Number(item.totalSize)) : "—"}
                      </TableCell>
                      <TableCell className="h-12 px-4">{formatDateTime(item.createdAt)}</TableCell>
                      <TableCell className="h-12 px-4">
                        <div className="flex items-center gap-1">
                          {enableNavigation && (
                            <Button
                              size="icon"
                              variant="ghost"
                              className="h-8 w-8 hover:bg-muted"
                              onClick={() => handleFolderClick(item.id)}
                              title={t("files.openFolder")}
                            >
                              <IconFolder className="h-4 w-4" />
                              <span className="sr-only">{t("files.openFolder")}</span>
                            </Button>
                          )}
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8 hover:bg-muted"
                            onClick={() => handleFolderDownload(item.id, item.name)}
                            title={t("filesTable.actions.download")}
                          >
                            <IconDownload className="h-4 w-4" />
                            <span className="sr-only">Download folder</span>
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                  );
                } else {
                  const { icon: FileIcon, color } = getFileIcon(item.name);
                  return (
                    <TableRow key={`file-${item.id}`} className="hover:bg-muted/50 transition-colors border-0">
                      <TableCell className="h-12 px-4 border-0">
                        <div className="flex items-center gap-2">
                          <FileIcon className={`h-5 w-5 ${color}`} />
                          <span className="truncate max-w-[250px] font-medium">{item.name}</span>
                        </div>
                      </TableCell>
                      <TableCell className="h-12 px-4">{formatFileSize(Number(item.size))}</TableCell>
                      <TableCell className="h-12 px-4">{formatDateTime(item.createdAt)}</TableCell>
                      <TableCell className="h-12 px-4">
                        <div className="flex items-center gap-1">
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8 hover:bg-muted"
                            onClick={() => handlePreview({ name: item.name, objectName: item.objectName })}
                          >
                            <IconEye className="h-4 w-4" />
                            <span className="sr-only">{t("filesTable.actions.preview")}</span>
                          </Button>
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8 hover:bg-muted"
                            onClick={() => onDownload(item.objectName, item.name)}
                          >
                            <IconDownload className="h-4 w-4" />
                            <span className="sr-only">{t("filesTable.actions.download")}</span>
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                  );
                }
              })
            )}
          </TableBody>
        </Table>
      </div>

      {selectedFile && (
        <FilePreviewModal
          isOpen={isPreviewOpen}
          onClose={handleClosePreview}
          file={selectedFile}
          sharePassword={sharePassword}
        />
      )}
    </div>
  );
}

export const ShareContentTable = ShareFilesTable;
