import { useState } from "react";
import { useTranslations } from "next-intl";

import { BulkDownloadModal } from "@/components/modals/bulk-download-modal";
import { CreateShareModal } from "@/components/modals/create-share-modal";
import { DeleteConfirmationModal } from "@/components/modals/delete-confirmation-modal";
import { FileActionsModals } from "@/components/modals/file-actions-modals";
import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import { GenerateShareLinkModal } from "@/components/modals/generate-share-link-modal";
import { ShareActionsModals } from "@/components/modals/share-actions-modals";
import { ShareDetailsModal } from "@/components/modals/share-details-modal";
import { ShareExpirationModal } from "@/components/modals/share-expiration-modal";
import { ShareFileModal } from "@/components/modals/share-file-modal";
import { ShareMultipleFilesModal } from "@/components/modals/share-multiple-files-modal";
import { ShareSecurityModal } from "@/components/modals/share-security-modal";
import { UploadFileModal } from "@/components/modals/upload-file-modal";
import { DashboardModalsProps } from "../types";

export function DashboardModals({ modals, fileManager, shareManager, onSuccess }: DashboardModalsProps) {
  const t = useTranslations();
  const [shareDetailsRefresh, setShareDetailsRefresh] = useState(0);

  const handleShareSuccess = () => {
    setShareDetailsRefresh((prev) => prev + 1);
    onSuccess();
  };

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

      <DeleteConfirmationModal
        isOpen={!!shareManager.sharesToDelete}
        onClose={() => shareManager.setSharesToDelete(null)}
        onConfirm={shareManager.handleDeleteBulk}
        title={t("shareActions.bulkDeleteTitle")}
        description={t("shareActions.bulkDeleteConfirmation", { count: shareManager.sharesToDelete?.length || 0 })}
        files={shareManager.sharesToDelete?.map((share: any) => share.name) || []}
        itemType="shares"
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

      <CreateShareModal isOpen={modals.isCreateModalOpen} onClose={modals.onCloseCreateModal} onSuccess={onSuccess} />

      <ShareActionsModals
        shareToDelete={shareManager.shareToDelete}
        shareToEdit={shareManager.shareToEdit}
        shareToManageFiles={shareManager.shareToManageFiles}
        shareToManageRecipients={shareManager.shareToManageRecipients}
        onCloseDelete={() => shareManager.setShareToDelete(null)}
        onCloseEdit={() => shareManager.setShareToEdit(null)}
        onCloseManageFiles={() => shareManager.setShareToManageFiles(null)}
        onCloseManageRecipients={() => shareManager.setShareToManageRecipients(null)}
        onDelete={shareManager.handleDelete}
        onEdit={shareManager.handleEdit}
        onManageFiles={shareManager.handleManageFiles}
        onManageRecipients={shareManager.handleManageRecipients}
        onEditFile={fileManager.handleRename}
        onSuccess={handleShareSuccess}
      />

      <ShareDetailsModal
        shareId={shareManager.shareToViewDetails?.id || null}
        onClose={() => shareManager.setShareToViewDetails(null)}
        onUpdateName={shareManager.handleUpdateName}
        onUpdateDescription={shareManager.handleUpdateDescription}
        onUpdateSecurity={async () => shareManager.handleUpdateSecurity(shareManager.shareToViewDetails!)}
        onUpdateExpiration={async () => shareManager.handleUpdateExpiration(shareManager.shareToViewDetails!)}
        onGenerateLink={shareManager.handleGenerateLink}
        onManageFiles={shareManager.setShareToManageFiles}
        refreshTrigger={shareDetailsRefresh}
        onSuccess={onSuccess}
      />

      <ShareSecurityModal
        shareId={shareManager.shareToManageSecurity?.id || null}
        share={shareManager.shareToManageSecurity || null}
        onClose={() => shareManager.setShareToManageSecurity(null)}
        onSuccess={onSuccess}
      />

      <ShareExpirationModal
        shareId={shareManager.shareToManageExpiration?.id || null}
        share={shareManager.shareToManageExpiration || null}
        onClose={() => shareManager.setShareToManageExpiration(null)}
        onSuccess={onSuccess}
      />

      <GenerateShareLinkModal
        share={shareManager.shareToGenerateLink || null}
        shareId={shareManager.shareToGenerateLink?.id || null}
        onClose={() => shareManager.setShareToGenerateLink(null)}
        onGenerate={shareManager.handleGenerateLink}
        onSuccess={onSuccess}
      />
    </>
  );
}
