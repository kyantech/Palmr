import { useState } from "react";
import { IconDownload, IconFolderOff, IconShare } from "@tabler/icons-react";
import { format } from "date-fns";
import { useTranslations } from "next-intl";

import { FilesViewManager } from "@/app/files/components/files-view-manager";
import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
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
  folders: Folder[];
  files: File[];
  path: Folder[];
  isBrowseLoading: boolean;
  searchQuery: string;
  navigateToFolder: (folderId?: string) => void;
  handleSearch: (query: string) => void;
}

export function ShareDetails({
  share,
  password,
  onDownload,
  onBulkDownload,
  onSelectedItemsBulkDownload,
  folders,
  files,
  path,
  isBrowseLoading,
  searchQuery,
  navigateToFolder,
  handleSearch,
}: ShareDetailsPropsExtended) {
  const t = useTranslations();
  const [isPreviewOpen, setIsPreviewOpen] = useState(false);
  const [selectedFile, setSelectedFile] = useState<{ name: string; objectName: string; type?: string } | null>(null);

  const shareHasItems = (share.files && share.files.length > 0) || (share.folders && share.folders.length > 0);
  const totalShareItems = (share.files?.length || 0) + (share.folders?.length || 0);
  const hasMultipleFiles = totalShareItems > 1;

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
              isLoading={isBrowseLoading}
              isShareMode={true}
              emptyStateComponent={() => (
                <div className="text-center py-16">
                  <div className="flex justify-center mb-6">
                    <IconFolderOff className="h-24 w-24 text-muted-foreground/30" />
                  </div>
                  <h3 className="text-lg font-semibold text-foreground mb-2">{t("fileSelector.noFilesInShare")}</h3>
                  <p className="text-muted-foreground max-w-sm mx-auto">{t("files.empty.description")}</p>
                </div>
              )}
              breadcrumbs={
                <Breadcrumb>
                  <BreadcrumbList>
                    <BreadcrumbItem>
                      <BreadcrumbLink
                        className="flex items-center gap-1 cursor-pointer"
                        onClick={() => navigateToFolder()}
                      >
                        <IconShare size={16} />
                        {t("folderActions.rootFolder")}
                      </BreadcrumbLink>
                    </BreadcrumbItem>

                    {path.map((folder, index) => (
                      <div key={folder.id} className="contents">
                        <BreadcrumbSeparator />
                        <BreadcrumbItem>
                          {index === path.length - 1 ? (
                            <BreadcrumbPage>{folder.name}</BreadcrumbPage>
                          ) : (
                            <BreadcrumbLink className="cursor-pointer" onClick={() => navigateToFolder(folder.id)}>
                              {folder.name}
                            </BreadcrumbLink>
                          )}
                        </BreadcrumbItem>
                      </div>
                    ))}
                  </BreadcrumbList>
                </Breadcrumb>
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
          sharePassword={password}
        />
      )}
    </>
  );
}
