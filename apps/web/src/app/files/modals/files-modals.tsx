import { BulkDownloadModal } from "@/components/modals/bulk-download-modal";
import { DeleteConfirmationModal } from "@/components/modals/delete-confirmation-modal";
import { FileActionsModals } from "@/components/modals/file-actions-modals";
import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import { ShareFileModal } from "@/components/modals/share-file-modal";
import { ShareMultipleFilesModal } from "@/components/modals/share-multiple-files-modal";
import { UploadFileModal } from "@/components/modals/upload-file-modal";
import type { FilesModalsProps } from "../types";

export function FilesModals({ fileManager, modals, onSuccess }: FilesModalsProps) {
  return (
    <>
      <UploadFileModal isOpen={modals.isUploadModalOpen} onClose={modals.onCloseUploadModal} onSuccess={onSuccess} />

      <FilePreviewModal
        file={fileManager.previewFile || { name: "", objectName: "" }}
        isOpen={!!fileManager.previewFile}
        onClose={() => fileManager.setPreviewFile(null)}
      />

      <ShareFileModal
        file={fileManager.fileToShare}
        isOpen={!!fileManager.fileToShare}
        onClose={() => fileManager.setFileToShare(null)}
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
        fileCount={fileManager.filesToDownload?.length || 0}
      />

      <DeleteConfirmationModal
        isOpen={!!fileManager.filesToDelete}
        onClose={() => fileManager.setFilesToDelete(null)}
        onConfirm={fileManager.handleDeleteBulk}
        title="Excluir Arquivos Selecionados"
        description={`Tem certeza que deseja excluir ${fileManager.filesToDelete?.length || 0} arquivo(s)? Esta ação não pode ser desfeita.`}
        files={fileManager.filesToDelete?.map((f) => f.name) || []}
      />

      <ShareMultipleFilesModal
        files={fileManager.filesToShare}
        isOpen={!!fileManager.filesToShare}
        onClose={() => fileManager.setFilesToShare(null)}
        onSuccess={() => {
          fileManager.handleShareBulkSuccess();
          onSuccess();
        }}
      />
    </>
  );
}
