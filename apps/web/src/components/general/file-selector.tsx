"use client";

import { useCallback, useEffect, useState } from "react";
import { IconCheck, IconEdit, IconEye, IconFolder, IconMinus, IconPlus, IconSearch } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { FileActionsModals } from "@/components/modals/file-actions-modals";
import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import { FolderActionsModals } from "@/components/modals/folder-actions-modals";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { addFiles, addFolders, listFiles, removeFiles, removeFolders } from "@/http/endpoints";
import { listFolders } from "@/http/endpoints/folders";
import { getFileIcon } from "@/utils/file-icons";

interface FileSelectorProps {
  shareId: string;
  selectedFiles: string[];
  selectedFolders?: string[];
  onSave: (files: string[], folders: string[]) => Promise<void>;
  onEditFile?: (fileId: string, newName: string, description?: string) => Promise<void>;
  onEditFolder?: (folderId: string, newName: string, description?: string) => Promise<void>;
}

export function FileSelector({
  shareId,
  selectedFiles,
  selectedFolders = [],
  onSave,
  onEditFile,
  onEditFolder,
}: FileSelectorProps) {
  const t = useTranslations();
  const [availableFiles, setAvailableFiles] = useState<any[]>([]);
  const [shareFiles, setShareFiles] = useState<any[]>([]);
  const [availableFolders, setAvailableFolders] = useState<any[]>([]);
  const [shareFolders, setShareFolders] = useState<any[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [searchFilter, setSearchFilter] = useState("");
  const [shareSearchFilter, setShareSearchFilter] = useState("");
  const [previewFile, setPreviewFile] = useState<any>(null);
  const [fileToEdit, setFileToEdit] = useState<any>(null);
  const [folderToEdit, setFolderToEdit] = useState<any>(null);

  const loadFiles = useCallback(async () => {
    try {
      const filesResponse = await listFiles();
      const allFiles = filesResponse.data.files || [];

      setShareFiles(allFiles.filter((file) => selectedFiles.includes(file.id)));
      setAvailableFiles(allFiles.filter((file) => !selectedFiles.includes(file.id)));

      const foldersResponse = await listFolders();
      const allFolders = foldersResponse.data.folders || [];

      setShareFolders(allFolders.filter((folder) => selectedFolders.includes(folder.id)));
      setAvailableFolders(allFolders.filter((folder) => !selectedFolders.includes(folder.id)));
    } catch {
      toast.error(t("files.loadError"));
    }
  }, [selectedFiles, selectedFolders, t]);

  useEffect(() => {
    loadFiles();
  }, [loadFiles]);

  const addToShare = (fileId: string) => {
    const file = availableFiles.find((f) => f.id === fileId);
    if (file) {
      setShareFiles([...shareFiles, file]);
      setAvailableFiles(availableFiles.filter((f) => f.id !== fileId));
    }
  };

  const removeFromShare = (fileId: string) => {
    const file = shareFiles.find((f) => f.id === fileId);
    if (file) {
      setAvailableFiles([...availableFiles, file]);
      setShareFiles(shareFiles.filter((f) => f.id !== fileId));
    }
  };

  const addFolderToShare = (folderId: string) => {
    const folder = availableFolders.find((f) => f.id === folderId);
    if (folder) {
      setShareFolders([...shareFolders, folder]);
      setAvailableFolders(availableFolders.filter((f) => f.id !== folderId));
    }
  };

  const removeFolderFromShare = (folderId: string) => {
    const folder = shareFolders.find((f) => f.id === folderId);
    if (folder) {
      setAvailableFolders([...availableFolders, folder]);
      setShareFolders(shareFolders.filter((f) => f.id !== folderId));
    }
  };

  const handleSave = async () => {
    try {
      setIsLoading(true);

      const filesToAdd = shareFiles.filter((file) => !selectedFiles.includes(file.id)).map((file) => file.id);
      const filesToRemove = selectedFiles.filter((fileId) => !shareFiles.find((f) => f.id === fileId));

      const foldersToAdd = shareFolders
        .filter((folder) => !selectedFolders.includes(folder.id))
        .map((folder) => folder.id);
      const foldersToRemove = selectedFolders.filter((folderId) => !shareFolders.find((f) => f.id === folderId));

      if (filesToAdd.length > 0) {
        await addFiles(shareId, { files: filesToAdd });
      }

      if (filesToRemove.length > 0) {
        await removeFiles(shareId, { files: filesToRemove });
      }

      if (foldersToAdd.length > 0) {
        await addFolders(shareId, { folders: foldersToAdd });
      }

      if (foldersToRemove.length > 0) {
        await removeFolders(shareId, { folders: foldersToRemove });
      }

      await onSave(
        shareFiles.map((f) => f.id),
        shareFolders.map((f) => f.id)
      );
    } catch {
      toast.error(t("shareManager.filesUpdateError"));
    } finally {
      setIsLoading(false);
    }
  };

  const handleEditFile = async (fileId: string, newName: string, description?: string) => {
    if (onEditFile) {
      await onEditFile(fileId, newName, description);
      setFileToEdit(null);
      await loadFiles();
    }
  };

  const handleEditFolder = async (folderId: string, newName: string, description?: string) => {
    if (onEditFolder) {
      await onEditFolder(folderId, newName, description);
      setFolderToEdit(null);
      await loadFiles();
    }
  };

  const filteredAvailableFiles = availableFiles.filter((file) =>
    file.name.toLowerCase().includes(searchFilter.toLowerCase())
  );

  const filteredShareFiles = shareFiles.filter((file) =>
    file.name.toLowerCase().includes(shareSearchFilter.toLowerCase())
  );

  const filteredAvailableFolders = availableFolders.filter((folder) =>
    folder.name.toLowerCase().includes(searchFilter.toLowerCase())
  );

  const filteredShareFolders = shareFolders.filter((folder) =>
    folder.name.toLowerCase().includes(shareSearchFilter.toLowerCase())
  );

  const formatFileSize = (bytes: number) => {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
  };

  const FileCard = ({ file, isInShare }: { file: any; isInShare: boolean }) => {
    const { icon: FileIcon, color } = getFileIcon(file.name);

    return (
      <div className="flex items-center gap-3 p-3 bg-background rounded-lg border group hover:border-muted-foreground/20 transition-colors">
        <FileIcon className={`h-5 w-5 ${color} flex-shrink-0`} />
        <div className="flex-1 min-w-0">
          <div className="font-medium text-sm truncate max-w-[260px]" title={file.name}>
            {file.name}
          </div>
          {file.description && (
            <div className="text-xs text-muted-foreground truncate max-w-[260px]" title={file.description}>
              {file.description}
            </div>
          )}
          <div className="text-xs text-muted-foreground">{formatFileSize(file.size)}</div>
        </div>
        <div className="flex items-center gap-2">
          <div className="flex gap-1">
            {onEditFile && (
              <Button
                size="icon"
                variant="ghost"
                className="h-7 w-7 hover:bg-muted transition-colors"
                onClick={() => setFileToEdit(file)}
                title={t("fileSelector.editFile")}
              >
                <IconEdit className="h-4 w-4" />
              </Button>
            )}
            <Button
              size="icon"
              variant="ghost"
              className="h-7 w-7 hover:bg-muted transition-colors"
              onClick={() => setPreviewFile(file)}
              title={t("fileSelector.previewFile")}
            >
              <IconEye className="h-4 w-4" />
            </Button>
          </div>

          <div className="ml-1">
            <Button
              size="icon"
              variant={isInShare ? "destructive" : "default"}
              className="h-8 w-8 transition-all"
              onClick={() => (isInShare ? removeFromShare(file.id) : addToShare(file.id))}
              title={isInShare ? t("fileSelector.removeFromShare") : t("fileSelector.addToShare")}
            >
              {isInShare ? <IconMinus className="h-4 w-4" /> : <IconPlus className="h-4 w-4" />}
            </Button>
          </div>
        </div>
      </div>
    );
  };

  const FolderCard = ({ folder, isInShare }: { folder: any; isInShare: boolean }) => {
    const formatFileSize = (bytes: string | number) => {
      const numBytes = typeof bytes === "string" ? parseInt(bytes) : bytes;
      if (numBytes === 0) return "0 B";
      const k = 1024;
      const sizes = ["B", "KB", "MB", "GB"];
      const i = Math.floor(Math.log(numBytes) / Math.log(k));
      return parseFloat((numBytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
    };

    return (
      <div className="flex items-center gap-3 p-3 bg-background rounded-lg border group hover:border-muted-foreground/20 transition-colors">
        <IconFolder className="h-5 w-5 text-blue-600 flex-shrink-0" />
        <div className="flex-1 min-w-0">
          <div className="font-medium text-sm truncate max-w-[260px]" title={folder.name}>
            {folder.name}
          </div>
          {folder.description && (
            <div className="text-xs text-muted-foreground truncate max-w-[260px]" title={folder.description}>
              {folder.description}
            </div>
          )}
          <div className="text-xs text-muted-foreground">
            {folder.totalSize ? formatFileSize(folder.totalSize) : "—"} • {folder._count?.files || 0} files
          </div>
        </div>
        <div className="flex items-center gap-2">
          <div className="flex gap-1">
            {onEditFolder && (
              <Button
                size="icon"
                variant="ghost"
                className="h-7 w-7 hover:bg-muted transition-colors"
                onClick={() => setFolderToEdit(folder)}
                title={t("fileSelector.editFolder")}
              >
                <IconEdit className="h-4 w-4" />
              </Button>
            )}
          </div>

          <div className="ml-1">
            <Button
              size="icon"
              variant={isInShare ? "destructive" : "default"}
              className="h-8 w-8 transition-all"
              onClick={() => (isInShare ? removeFolderFromShare(folder.id) : addFolderToShare(folder.id))}
              title={isInShare ? "Remove from share" : "Add to share"}
            >
              {isInShare ? <IconMinus className="h-4 w-4" /> : <IconPlus className="h-4 w-4" />}
            </Button>
          </div>
        </div>
      </div>
    );
  };

  return (
    <>
      <div className="space-y-6">
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-lg font-medium">Share Contents ({shareFiles.length + shareFolders.length} items)</h3>
              <p className="text-sm text-muted-foreground">Files and folders that will be shared</p>
            </div>
            <Badge variant="secondary" className="bg-blue-500/20 text-blue-700">
              {shareFiles.length} files • {shareFolders.length} folders
            </Badge>
          </div>

          {(shareFiles.length > 0 || shareFolders.length > 0) && (
            <div className="relative">
              <IconSearch className="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-muted-foreground" />
              <Input
                placeholder={t("searchBar.placeholder")}
                value={shareSearchFilter}
                onChange={(e) => setShareSearchFilter(e.target.value)}
                className="pl-10"
              />
            </div>
          )}

          {shareFiles.length > 0 || shareFolders.length > 0 ? (
            <div className="grid gap-2 max-h-40 overflow-y-auto border rounded-lg p-3 bg-muted/30">
              {filteredShareFolders.map((folder) => (
                <FolderCard key={`folder-${folder.id}`} folder={folder} isInShare={true} />
              ))}
              {filteredShareFiles.map((file) => (
                <FileCard key={`file-${file.id}`} file={file} isInShare={true} />
              ))}
              {filteredShareFiles.length === 0 && filteredShareFolders.length === 0 && shareSearchFilter && (
                <div className="text-center py-4 text-muted-foreground">
                  <p className="text-sm">No items found with "{shareSearchFilter}"</p>
                </div>
              )}
            </div>
          ) : (
            <div className="text-center py-8 text-muted-foreground border rounded-lg bg-muted/20">
              <div className="text-4xl mb-2">📁</div>
              <p className="font-medium">{t("fileSelector.noFilesInShare")}</p>
              <p className="text-sm">{t("fileSelector.addFilesFromList")}</p>
            </div>
          )}
        </div>

        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-lg font-medium">
                Available Items ({filteredAvailableFiles.length + filteredAvailableFolders.length})
              </h3>
              <p className="text-sm text-muted-foreground">Files and folders you can add to the share</p>
            </div>
          </div>

          <div className="relative">
            <IconSearch className="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder={t("fileSelector.searchPlaceholder")}
              value={searchFilter}
              onChange={(e) => setSearchFilter(e.target.value)}
              className="pl-10"
            />
          </div>

          {filteredAvailableFiles.length > 0 || filteredAvailableFolders.length > 0 ? (
            <div className="grid gap-2 max-h-60 overflow-y-auto border rounded-lg p-3 bg-muted/10">
              {filteredAvailableFolders.map((folder) => (
                <FolderCard key={`folder-${folder.id}`} folder={folder} isInShare={false} />
              ))}
              {filteredAvailableFiles.map((file) => (
                <FileCard key={`file-${file.id}`} file={file} isInShare={false} />
              ))}
            </div>
          ) : searchFilter ? (
            <div className="text-center py-8 text-muted-foreground border rounded-lg bg-muted/20">
              <div className="text-4xl mb-2">🔍</div>
              <p className="font-medium">{t("fileSelector.noFilesFound")}</p>
              <p className="text-sm">{t("fileSelector.tryDifferentSearch")}</p>
            </div>
          ) : (
            <div className="text-center py-8 text-muted-foreground border rounded-lg bg-muted/20">
              <div className="text-4xl mb-2">📄</div>
              <p className="font-medium">{t("fileSelector.allFilesInShare")}</p>
              <p className="text-sm">{t("fileSelector.uploadNewFiles")}</p>
            </div>
          )}
        </div>

        <div className="flex items-center justify-between pt-4 border-t">
          <div className="text-sm text-muted-foreground">{shareFiles.length + shareFolders.length} items selected</div>
          <Button onClick={handleSave} disabled={isLoading} className="gap-2">
            {isLoading ? (
              <>
                <div className="animate-spin h-4 w-4 border-2 border-current border-t-transparent rounded-full" />
                {t("common.saving")}
              </>
            ) : (
              <>
                <IconCheck className="h-4 w-4" />
                {t("fileSelector.saveChanges")}
              </>
            )}
          </Button>
        </div>
      </div>

      <FilePreviewModal
        isOpen={!!previewFile}
        onClose={() => setPreviewFile(null)}
        file={previewFile || { name: "", objectName: "" }}
      />

      <FileActionsModals
        fileToRename={fileToEdit}
        fileToDelete={null}
        onRename={handleEditFile}
        onDelete={async () => {}}
        onCloseRename={() => setFileToEdit(null)}
        onCloseDelete={() => {}}
      />

      <FolderActionsModals
        folderToCreate={false}
        onCreateFolder={async () => {}}
        onCloseCreate={() => {}}
        folderToEdit={folderToEdit}
        onEditFolder={handleEditFolder}
        onCloseEdit={() => setFolderToEdit(null)}
        folderToDelete={null}
        onDeleteFolder={async () => {}}
        onCloseDelete={() => {}}
      />
    </>
  );
}
