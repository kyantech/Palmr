import { useTranslations } from "next-intl";

import { FolderActionsModals } from "@/components/modals";
import { BulkDownloadModal } from "@/components/modals/bulk-download-modal";
import { DeleteConfirmationModal } from "@/components/modals/delete-confirmation-modal";
import { FileActionsModals } from "@/components/modals/file-actions-modals";
import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import { ShareItemModal } from "@/components/modals/share-item-modal";
import { ShareMultipleItemsModal } from "@/components/modals/share-multiple-items-modal";
import { UploadFileModal } from "@/components/modals/upload-file-modal";
import type { FilesModalsProps } from "../types";

export function FilesModals({
  fileManager,
  modals,
  onSuccess,
  currentFolderId,
}: FilesModalsProps & { currentFolderId?: string | null }) {
  const t = useTranslations();

  return (
    <>
      <UploadFileModal
        isOpen={modals.isUploadModalOpen}
        onClose={modals.onCloseUploadModal}
        onSuccess={onSuccess}
        currentFolderId={currentFolderId || undefined}
      />

      {/* Folder Modals */}
      <FolderActionsModals
        folderToCreate={fileManager.isCreateFolderModalOpen}
        onCloseCreate={() => fileManager.setCreateFolderModalOpen(false)}
        onCreateFolder={(name, description) =>
          fileManager.handleCreateFolder({ name, description }, currentFolderId || undefined)
        }
        folderToEdit={fileManager.folderToRename}
        onCloseEdit={() => fileManager.setFolderToRename(null)}
        onEditFolder={fileManager.handleFolderRename}
        folderToDelete={fileManager.folderToDelete}
        onCloseDelete={() => fileManager.setFolderToDelete(null)}
        onDeleteFolder={fileManager.handleFolderDelete}
      />

      <FilePreviewModal
        file={fileManager.previewFile || { name: "", objectName: "" }}
        isOpen={!!fileManager.previewFile}
        onClose={() => fileManager.setPreviewFile(null)}
      />

      <ShareItemModal
        file={fileManager.fileToShare}
        folder={fileManager.folderToShare}
        isOpen={!!(fileManager.fileToShare || fileManager.folderToShare)}
        onClose={() => {
          fileManager.setFileToShare(null);
          fileManager.setFolderToShare(null);
        }}
        onSuccess={onSuccess}
      />

      <FileActionsModals
        fileToDelete={fileManager.fileToDelete}
        fileToRename={fileManager.fileToRename}
        onCloseDelete={() => fileManager.setFileToDelete(null)}
        onCloseRename={() => fileManager.setFileToRename(null)}
        onDelete={fileManager.handleDelete}
        onRename={fileManager.handleRename}
      />

      {/* Bulk Actions Modals */}
      <BulkDownloadModal
        isOpen={fileManager.isBulkDownloadModalOpen}
        onClose={() => fileManager.setBulkDownloadModalOpen(false)}
        onDownload={(zipName) => {
          if (fileManager.filesToDownload) {
            fileManager.handleBulkDownloadWithZip(fileManager.filesToDownload, zipName);
          }
        }}
        items={[
          ...(fileManager.filesToDownload?.map((file) => ({
            id: file.id,
            name: file.name,
            size: file.size,
            type: "file" as const,
          })) || []),
          ...(fileManager.foldersToDownload?.map((folder) => ({
            id: folder.id,
            name: folder.name,
            size: folder.totalSize ? parseInt(folder.totalSize) : undefined,
            type: "folder" as const,
          })) || []),
        ]}
      />

      <DeleteConfirmationModal
        isOpen={!!(fileManager.filesToDelete || fileManager.foldersToDelete)}
        onClose={() => {
          fileManager.setFilesToDelete(null);
          fileManager.setFoldersToDelete(null);
        }}
        onConfirm={fileManager.handleDeleteBulk}
        title={t("files.bulkDeleteTitle")}
        description={t("files.bulkDeleteConfirmation", {
          count: (fileManager.filesToDelete?.length || 0) + (fileManager.foldersToDelete?.length || 0),
        })}
        files={fileManager.filesToDelete?.map((f) => f.name) || []}
        folders={fileManager.foldersToDelete?.map((f) => f.name) || []}
        itemType={
          (fileManager.filesToDelete?.length || 0) > 0 && (fileManager.foldersToDelete?.length || 0) > 0
            ? "mixed"
            : (fileManager.foldersToDelete?.length || 0) > 0
              ? "files"
              : "files"
        }
      />

      <ShareMultipleItemsModal
        files={fileManager.filesToShare}
        folders={fileManager.foldersToShare}
        isOpen={!!(fileManager.filesToShare || fileManager.foldersToShare)}
        onClose={() => {
          fileManager.setFilesToShare(null);
          fileManager.setFoldersToShare(null);
        }}
        onSuccess={() => {
          fileManager.handleShareBulkSuccess();
          onSuccess();
        }}
      />
    </>
  );
}
