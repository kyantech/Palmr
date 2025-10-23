"use client";

import { useState } from "react";
import { IconFolderOpen } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { ProtectedRoute } from "@/components/auth/protected-route";
import { GlobalDropZone } from "@/components/general/global-drop-zone";
import { FileManagerLayout } from "@/components/layout/file-manager-layout";
import { MoveItemsModal } from "@/components/modals/move-items-modal";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Card, CardContent } from "@/components/ui/card";
import { moveFile } from "@/http/endpoints/files";
import { listFolders, moveFolder } from "@/http/endpoints/folders";
import { FilesViewManager } from "./components/files-view-manager";
import { Header } from "./components/header";
import { useFileBrowser } from "./hooks/use-file-browser";
import { FilesModals } from "./modals/files-modals";

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

export default function FilesPage() {
  const t = useTranslations();
  const [itemsToMove, setItemsToMove] = useState<{ files: File[]; folders: Folder[] } | null>(null);

  const {
    isLoading,
    searchQuery,
    currentPath,
    fileManager,
    filteredFiles,
    filteredFolders,
    navigateToFolder,
    navigateToRoot,
    handleSearch,
    loadFiles,
    modals,
  } = useFileBrowser();

  const handleMoveFile = (file: any) => {
    setItemsToMove({ files: [file], folders: [] });
  };

  const handleMoveFolder = (folder: any) => {
    setItemsToMove({ files: [], folders: [folder] });
  };

  const handleBulkMove = (files: File[], folders: Folder[]) => {
    setItemsToMove({ files, folders });
  };

  const handleMove = async (targetFolderId: string | null) => {
    if (!itemsToMove) return;

    try {
      if (itemsToMove.files.length > 0) {
        await Promise.all(itemsToMove.files.map((file) => moveFile(file.id, { folderId: targetFolderId })));
      }

      if (itemsToMove.folders.length > 0) {
        await Promise.all(itemsToMove.folders.map((folder) => moveFolder(folder.id, { parentId: targetFolderId })));
      }

      const itemCount = itemsToMove.files.length + itemsToMove.folders.length;
      toast.success(t("moveItems.success", { count: itemCount }));

      await loadFiles();
      setItemsToMove(null);
    } catch (error) {
      console.error("Error moving items:", error);
      toast.error("Failed to move items. Please try again.");
    }
  };

  const handleUploadSuccess = async () => {
    await loadFiles();
    toast.success("Files uploaded successfully");
  };

  return (
    <ProtectedRoute>
      <GlobalDropZone
        onSuccess={loadFiles}
        currentFolderId={currentPath.length > 0 ? currentPath[currentPath.length - 1].id : null}
      >
        <FileManagerLayout
          breadcrumbLabel={t("files.breadcrumb")}
          icon={<IconFolderOpen size={20} />}
          title={t("files.pageTitle")}
        >
          <Card>
            <CardContent>
              <div className="flex flex-col gap-6">
                <Header
                  onUpload={modals.onOpenUploadModal}
                  onCreateFolder={() => fileManager.setCreateFolderModalOpen(true)}
                />

                <FilesViewManager
                  files={filteredFiles}
                  folders={filteredFolders}
                  searchQuery={searchQuery}
                  onSearch={handleSearch}
                  onDownload={fileManager.handleDownload}
                  isLoading={isLoading}
                  breadcrumbs={
                    <Breadcrumb>
                      <BreadcrumbList>
                        <BreadcrumbItem>
                          <BreadcrumbLink className="flex items-center gap-1 cursor-pointer" onClick={navigateToRoot}>
                            <IconFolderOpen size={16} />
                            {t("folderActions.rootFolder")}
                          </BreadcrumbLink>
                        </BreadcrumbItem>

                        {currentPath.map((folder, index) => (
                          <div key={folder.id} className="contents">
                            <BreadcrumbSeparator />
                            <BreadcrumbItem>
                              {index === currentPath.length - 1 ? (
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
                  onDeleteFolder={(folder) =>
                    fileManager.setFolderToDelete({
                      id: folder.id,
                      name: folder.name,
                    })
                  }
                  onRenameFolder={(folder) =>
                    fileManager.setFolderToRename({
                      id: folder.id,
                      name: folder.name,
                      description: folder.description || undefined,
                    })
                  }
                  onMoveFolder={handleMoveFolder}
                  onMoveFile={handleMoveFile}
                  onShareFolder={fileManager.setFolderToShare}
                  onDownloadFolder={fileManager.handleSingleFolderDownload}
                  onPreview={fileManager.setPreviewFile}
                  onRename={fileManager.setFileToRename}
                  onShare={fileManager.setFileToShare}
                  onDelete={fileManager.setFileToDelete}
                  onBulkDelete={(files, folders) => {
                    fileManager.handleBulkDelete(files, folders);
                  }}
                  onBulkShare={(files, folders) => {
                    // Use enhanced bulk share that handles both files and folders
                    fileManager.handleBulkShare(files, folders);
                  }}
                  onBulkDownload={(files, folders) => {
                    // Use enhanced bulk download that handles both files and folders
                    fileManager.handleBulkDownload(files, folders);
                  }}
                  onBulkMove={handleBulkMove}
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
                  onUpdateFolderName={(folderId, newName) => {
                    const folder = filteredFolders.find((f) => f.id === folderId);
                    if (folder) {
                      fileManager.handleFolderRename(folderId, newName, folder.description);
                    }
                  }}
                  onUpdateFolderDescription={(folderId, newDescription) => {
                    const folder = filteredFolders.find((f) => f.id === folderId);
                    if (folder) {
                      fileManager.handleFolderRename(folderId, folder.name, newDescription);
                    }
                  }}
                  emptyStateComponent={() => (
                    <div className="flex flex-col items-center justify-center py-12 text-center">
                      <IconFolderOpen size={48} className="text-muted-foreground mb-4" />
                      <h3 className="text-lg font-semibold mb-2">{t("files.empty.title")}</h3>
                      <p className="text-muted-foreground mb-6">{t("files.empty.description")}</p>
                    </div>
                  )}
                />
              </div>
            </CardContent>
          </Card>

          <FilesModals
            fileManager={fileManager}
            modals={modals}
            onSuccess={handleUploadSuccess}
            currentFolderId={currentPath.length > 0 ? currentPath[currentPath.length - 1].id : null}
          />

          <MoveItemsModal
            isOpen={!!itemsToMove}
            onClose={() => setItemsToMove(null)}
            onMove={handleMove}
            itemsToMove={itemsToMove}
            getAllFolders={async () => {
              const response = await listFolders();
              return response.data.folders || [];
            }}
            currentFolderId={currentPath.length > 0 ? currentPath[currentPath.length - 1].id : null}
          />
        </FileManagerLayout>
      </GlobalDropZone>
    </ProtectedRoute>
  );
}
