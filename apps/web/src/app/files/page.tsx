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
import { getCachedDownloadUrl } from "@/lib/download-url-cache";
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
    handleImmediateUpdate,
    modals,
    allFiles,
    allFolders,
    sortBy,
    sortOrder,
    setSortBy,
    setSortOrder,
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
      toast.error(t("files.errors.moveItemsFailed"));
    }
  };

  const handleUploadSuccess = async () => {
    await loadFiles();
    // Toast is already shown by the upload modal
  };

  const handleFolderDownload = async (folderId: string, folderName: string) => {
    try {
      // Get all files in this folder and subfolders recursively with their paths
      const getFolderFilesWithPath = (
        targetFolderId: string,
        currentPath: string = ""
      ): Array<{ file: File; path: string }> => {
        const filesWithPath: Array<{ file: File; path: string }> = [];

        // Get direct files in this folder
        const directFiles = allFiles.filter((f) => f.folderId === targetFolderId);
        directFiles.forEach((file) => {
          filesWithPath.push({ file, path: currentPath });
        });

        // Get subfolders and process them recursively
        const subfolders = allFolders.filter((f) => f.parentId === targetFolderId);
        for (const subfolder of subfolders) {
          const subfolderPath = currentPath ? `${currentPath}/${subfolder.name}` : subfolder.name;
          filesWithPath.push(...getFolderFilesWithPath(subfolder.id, subfolderPath));
        }

        return filesWithPath;
      };

      const folderFilesWithPath = getFolderFilesWithPath(folderId);

      if (folderFilesWithPath.length === 0) {
        toast.error(t("shareManager.noFilesToDownload"));
        return;
      }

      const loadingToast = toast.loading(t("shareManager.creatingZip"));

      try {
        // Get presigned URLs for all files with their relative paths
        const downloadItems = await Promise.all(
          folderFilesWithPath.map(async ({ file, path }) => {
            const url = await getCachedDownloadUrl(file.objectName);
            return {
              url,
              name: path ? `${path}/${file.name}` : file.name,
            };
          })
        );

        // Create ZIP with all files
        const { downloadFilesAsZip } = await import("@/utils/zip-download");
        const zipName = `${folderName}.zip`;
        await downloadFilesAsZip(downloadItems, zipName);

        toast.dismiss(loadingToast);
        toast.success(t("shareManager.zipDownloadSuccess"));
      } catch (error) {
        toast.dismiss(loadingToast);
        toast.error(t("shareManager.zipDownloadError"));
        throw error;
      }
    } catch (error) {
      console.error("Error downloading folder:", error);
      toast.error(t("share.errors.downloadFailed"));
    }
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
                  onCreateFolder={() => fileManager.setCreateFolderModalOpen(true)}
                  onUpload={modals.onOpenUploadModal}
                  sortBy={sortBy}
                  sortOrder={sortOrder}
                  onSortByChange={setSortBy}
                  onSortOrderChange={setSortOrder}
                  breadcrumbs={
                    <Breadcrumb>
                      <BreadcrumbList>
                        <BreadcrumbItem>
                          <BreadcrumbLink
                            className="flex items-center gap-1.5 cursor-pointer transition-colors p-0.5 rounded-md"
                            onClick={navigateToRoot}
                            onDragOver={(e) => {
                              e.preventDefault();
                              e.stopPropagation();
                              e.currentTarget.classList.add("bg-primary/10", "text-primary");
                            }}
                            onDragLeave={(e) => {
                              e.currentTarget.classList.remove("bg-primary/10", "text-primary");
                            }}
                            onDrop={async (e) => {
                              e.preventDefault();
                              e.stopPropagation();
                              e.currentTarget.classList.remove("bg-primary/10", "text-primary");

                              try {
                                const itemData = e.dataTransfer.getData("text/plain");
                                const items = JSON.parse(itemData);

                                // Update UI immediately
                                items.forEach((item: any) => {
                                  handleImmediateUpdate(item.id, item.type, null);
                                });

                                // Move all items in parallel
                                const movePromises = items.map((item: any) => {
                                  if (item.type === "file") {
                                    return moveFile(item.id, { folderId: null });
                                  } else if (item.type === "folder") {
                                    return moveFolder(item.id, { parentId: null });
                                  }
                                  return Promise.resolve();
                                });

                                await Promise.all(movePromises);

                                if (items.length === 1) {
                                  toast.success(
                                    `${items[0].type === "folder" ? "Folder" : "File"} "${items[0].name}" moved to root folder`
                                  );
                                } else {
                                  toast.success(`${items.length} items moved to root folder`);
                                }
                              } catch (error) {
                                console.error("Error moving items:", error);
                                toast.error(t("files.errors.moveItemsFailed"));
                                await loadFiles();
                              }
                            }}
                          >
                            <IconFolderOpen size={16} />
                            {t("folderActions.rootFolder")}
                          </BreadcrumbLink>
                        </BreadcrumbItem>

                        {currentPath.map((folder, index) => (
                          <div key={folder.id} className="contents">
                            <BreadcrumbSeparator />
                            <BreadcrumbItem>
                              {index === currentPath.length - 1 ? (
                                <BreadcrumbPage className="flex items-center gap-1.5">
                                  <IconFolderOpen size={16} />
                                  {folder.name}
                                </BreadcrumbPage>
                              ) : (
                                <BreadcrumbLink
                                  className="flex items-center gap-1 cursor-pointer transition-colors p-0.5 rounded-md"
                                  onClick={() => navigateToFolder(folder.id)}
                                  onDragOver={(e) => {
                                    e.preventDefault();
                                    e.stopPropagation();
                                    e.currentTarget.classList.add("bg-primary/10", "text-primary");
                                  }}
                                  onDragLeave={(e) => {
                                    e.currentTarget.classList.remove("bg-primary/10", "text-primary");
                                  }}
                                  onDrop={async (e) => {
                                    e.preventDefault();
                                    e.stopPropagation();
                                    e.currentTarget.classList.remove("bg-primary/10", "text-primary");

                                    try {
                                      const itemData = e.dataTransfer.getData("text/plain");
                                      const items = JSON.parse(itemData);

                                      // Filter out invalid moves
                                      const validItems = items.filter((item: any) => {
                                        if (item.id === folder.id) return false;
                                        if (item.type === "folder" && item.id === folder.id) return false;
                                        return true;
                                      });

                                      if (validItems.length === 0) {
                                        toast.error(t("files.errors.cannotMoveHere"));
                                        return;
                                      }

                                      // Update UI immediately
                                      validItems.forEach((item: any) => {
                                        handleImmediateUpdate(item.id, item.type, folder.id);
                                      });

                                      // Move all items in parallel
                                      const movePromises = validItems.map((item: any) => {
                                        if (item.type === "file") {
                                          return moveFile(item.id, { folderId: folder.id });
                                        } else if (item.type === "folder") {
                                          return moveFolder(item.id, { parentId: folder.id });
                                        }
                                        return Promise.resolve();
                                      });

                                      await Promise.all(movePromises);

                                      if (validItems.length === 1) {
                                        toast.success(
                                          `${validItems[0].type === "folder" ? "Folder" : "File"} "${validItems[0].name}" moved to "${folder.name}"`
                                        );
                                      } else {
                                        toast.success(`${validItems.length} items moved to "${folder.name}"`);
                                      }
                                    } catch (error) {
                                      console.error("Error moving items:", error);
                                      toast.error(t("files.errors.moveItemsFailed"));
                                      await loadFiles();
                                    }
                                  }}
                                >
                                  <IconFolderOpen size={16} />
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
                  onRefresh={loadFiles}
                  onImmediateUpdate={handleImmediateUpdate}
                  onShareFolder={fileManager.setFolderToShare}
                  onDownloadFolder={handleFolderDownload}
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
