import { useState } from "react";
import { IconDownload, IconShare } from "@tabler/icons-react";
import { format } from "date-fns";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { ShareEmptyState } from "@/app/(shares)/s/[alias]/components/browse/share-empty-state";
import { FilesViewManager } from "@/app/files/components/files-view-manager";
import { FolderBreadcrumbs } from "@/components/general/folder-breadcrumbs";
import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { getShare } from "@/http/endpoints/shares";
import { bulkDownloadWithQueue } from "@/utils/download-queue-utils";
import { useShareBrowse } from "../hooks/use-share-browse";
import { ShareDetailsProps } from "../types";

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

interface ShareDetailsPropsExtended extends Omit<ShareDetailsProps, "onBulkDownload"> {
  onBulkDownload?: () => Promise<void>;
  onSelectedItemsBulkDownload?: (files: File[], folders: Folder[]) => Promise<void>;
}

export function ShareDetails({
  share,
  password,
  onDownload,
  onBulkDownload,
  onSelectedItemsBulkDownload,
}: ShareDetailsPropsExtended) {
  const t = useTranslations();
  const [isPreviewOpen, setIsPreviewOpen] = useState(false);
  const [selectedFile, setSelectedFile] = useState<{ name: string; objectName: string; type?: string } | null>(null);

  // Check if share has any items (use the full share data, not current folder level)
  const shareHasItems = (share.files && share.files.length > 0) || (share.folders && share.folders.length > 0);
  const totalShareItems = (share.files?.length || 0) + (share.folders?.length || 0);
  const hasMultipleFiles = totalShareItems > 1;

  // Use the share browse hook for folder navigation
  const { folders, files, path, isLoading, searchQuery, navigateToFolder, handleSearch } = useShareBrowse({
    shareId: share.id,
    password: password,
  });

  const handleFolderDownload = async (folderId: string, folderName: string) => {
    // Use the download handler from the hook which uses toast.promise
    await onDownload(`folder:${folderId}`, folderName);
  };

  return (
    <>
      <Card>
        <CardContent>
          <div className="flex flex-col gap-6">
            <div className="flex flex-col gap-2">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <IconShare className="w-6 h-6 text-muted-foreground" />
                  <h1 className="text-2xl font-semibold">{share.name || t("share.details.untitled")}</h1>
                </div>
                {shareHasItems && hasMultipleFiles && (
                  <Button onClick={onBulkDownload} className="flex items-center gap-2">
                    <IconDownload className="w-4 h-4" />
                    {t("share.downloadAll")}
                  </Button>
                )}
              </div>
              {share.description && <p className="text-muted-foreground">{share.description}</p>}
              <div className="flex gap-4 text-sm text-muted-foreground">
                <span>
                  {t("share.details.created", {
                    date: format(new Date(share.createdAt), "MM/dd/yyyy HH:mm"),
                  })}
                </span>
                {share.expiration && (
                  <span>
                    {t("share.details.expires", {
                      date: format(new Date(share.expiration), "MM/dd/yyyy HH:mm"),
                    })}
                  </span>
                )}
              </div>
            </div>

            <FilesViewManager
              files={files}
              folders={folders}
              searchQuery={searchQuery}
              onSearch={handleSearch}
              onDownload={onDownload}
              onBulkDownload={onSelectedItemsBulkDownload}
              isLoading={isLoading}
              isShareMode={true}
              emptyStateComponent={ShareEmptyState}
              breadcrumbs={
                <FolderBreadcrumbs
                  currentPath={path}
                  onNavigate={navigateToFolder}
                  onNavigateToRoot={() => navigateToFolder()}
                />
              }
              onNavigateToFolder={navigateToFolder}
              onDownloadFolder={handleFolderDownload}
              onPreview={(file) => {
                setSelectedFile({ name: file.name, objectName: file.objectName });
                setIsPreviewOpen(true);
              }}
            />
          </div>
        </CardContent>
      </Card>

      {selectedFile && (
        <FilePreviewModal
          isOpen={isPreviewOpen}
          onClose={() => {
            setIsPreviewOpen(false);
            setSelectedFile(null);
          }}
          file={selectedFile}
        />
      )}
    </>
  );
}
