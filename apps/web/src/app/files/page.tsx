"use client";

import { IconFolderOpen } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { ProtectedRoute } from "@/components/auth/protected-route";
import { GlobalDropZone } from "@/components/general/global-drop-zone";
import { FileManagerLayout } from "@/components/layout/file-manager-layout";
import { LoadingScreen } from "@/components/layout/loading-screen";
import { Card, CardContent } from "@/components/ui/card";
import { FilesViewManager } from "./components/files-view-manager";
import { Header } from "./components/header";
import { useFiles } from "./hooks/use-files";
import { FilesModals } from "./modals/files-modals";

export default function FilesPage() {
  const t = useTranslations();

  const { isLoading, searchQuery, modals, fileManager, filteredFiles, handleSearch, loadFiles } = useFiles();

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <ProtectedRoute>
      <GlobalDropZone onSuccess={loadFiles}>
        <FileManagerLayout
          breadcrumbLabel={t("files.breadcrumb")}
          icon={<IconFolderOpen size={20} />}
          title={t("files.pageTitle")}
          pendingDownloads={fileManager.pendingDownloads}
          onCancelDownload={fileManager.cancelPendingDownload}
        >
          <Card>
            <CardContent>
              <div className="flex flex-col gap-6">
                <Header onUpload={modals.onOpenUploadModal} />
                <FilesViewManager
                  files={filteredFiles}
                  searchQuery={searchQuery}
                  onSearch={handleSearch}
                  onDelete={fileManager.setFileToDelete}
                  onDownload={fileManager.handleDownload}
                  onPreview={fileManager.setPreviewFile}
                  onRename={fileManager.setFileToRename}
                  onShare={fileManager.setFileToShare}
                  onBulkDelete={fileManager.handleBulkDelete}
                  onBulkShare={fileManager.handleBulkShare}
                  onBulkDownload={fileManager.handleBulkDownload}
                  setClearSelectionCallback={fileManager.setClearSelectionCallback}
                  onUpdateName={(fileId, newName) => {
                    const file = filteredFiles.find((f) => f.id === fileId);
                    if (file) {
                      fileManager.handleRename(fileId, newName, file.description);
                    }
                  }}
                  onUpdateDescription={(fileId, newDescription) => {
                    const file = filteredFiles.find((f) => f.id === fileId);
                    if (file) {
                      fileManager.handleRename(fileId, file.name, newDescription);
                    }
                  }}
                />
              </div>
            </CardContent>
          </Card>

          <FilesModals fileManager={fileManager} modals={modals} onSuccess={loadFiles} />
        </FileManagerLayout>
      </GlobalDropZone>
    </ProtectedRoute>
  );
}
