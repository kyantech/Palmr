import { useState } from "react";
import { useTranslations } from "next-intl";

import { BulkDownloadModal } from "@/components/modals/bulk-download-modal";
import { CreateShareModal } from "@/components/modals/create-share-modal";
import { DeleteConfirmationModal } from "@/components/modals/delete-confirmation-modal";
import { FileActionsModals } from "@/components/modals/file-actions-modals";
import { FilePreviewModal } from "@/components/modals/file-preview-modal";
import { GenerateShareLinkModal } from "@/components/modals/generate-share-link-modal";
import { QrCodeModal } from "@/components/modals/qr-code-modal";
import { ShareActionsModals } from "@/components/modals/share-actions-modals";
import { ShareDetailsModal } from "@/components/modals/share-details-modal";
import { ShareExpirationModal } from "@/components/modals/share-expiration-modal";
import { ShareItemModal } from "@/components/modals/share-item-modal";
import { ShareMultipleItemsModal } from "@/components/modals/share-multiple-items-modal";
import { ShareSecurityModal } from "@/components/modals/share-security-modal";
import { UploadFileModal } from "@/components/modals/upload-file-modal";
import { listFiles, listFolders } from "@/http/endpoints";
import { DashboardModalsProps } from "../types";

export function DashboardModals({
  modals,
  fileManager,
  shareManager,
  onSuccess,
  existingFiles,
}: DashboardModalsProps & { existingFiles?: { name: string; folderId?: string | null }[] }) {
  const t = useTranslations();
  const [shareDetailsRefresh, setShareDetailsRefresh] = useState(0);

  const handleShareSuccess = () => {
    setShareDetailsRefresh((prev) => prev + 1);
    onSuccess();
  };

  const getShareLink = (share: any) => {
    if (!share?.alias?.alias) return "";
    return `${window.location.origin}/s/${share.alias.alias}`;
  };

  return (
    <>
      <UploadFileModal
        isOpen={modals.isUploadModalOpen}
        onClose={modals.onCloseUploadModal}
        onSuccess={onSuccess}
        existingFiles={existingFiles}
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

      <BulkDownloadModal
        isOpen={fileManager.isBulkDownloadModalOpen}
        onClose={() => fileManager.setBulkDownloadModalOpen(false)}
        onDownload={(zipName) => {
          if (fileManager.filesToDownload || fileManager.foldersToDownload) {
            fileManager.handleBulkDownloadWithZip(fileManager.filesToDownload || [], zipName);
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
            size: folder.totalSize ? Number(folder.totalSize) : undefined,
            type: "folder" as const,
          })) || []),
        ]}
      />

      <DeleteConfirmationModal
        isOpen={!!fileManager.filesToDelete}
        onClose={() => fileManager.setFilesToDelete(null)}
        onConfirm={fileManager.handleDeleteBulk}
        title={t("files.bulkDeleteTitle")}
        description={t("files.bulkDeleteConfirmation", { count: fileManager.filesToDelete?.length || 0 })}
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

      <CreateShareModal
        isOpen={modals.isCreateModalOpen}
        onClose={modals.onCloseCreateModal}
        onSuccess={() => {
          modals.onCloseCreateModal();
          onSuccess();
        }}
        getAllFilesAndFolders={async () => {
          const [filesResponse, foldersResponse] = await Promise.all([listFiles(), listFolders()]);
          return {
            files: filesResponse.data.files || [],
            folders: foldersResponse.data.folders || [],
          };
        }}
      />

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
        onEditFolder={shareManager.handleEditFolder}
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

      <QrCodeModal
        isOpen={!!shareManager.shareToViewQrCode}
        onClose={() => shareManager.setShareToViewQrCode(null)}
        shareLink={getShareLink(shareManager.shareToViewQrCode)}
        shareName={shareManager.shareToViewQrCode?.name || "Share"}
      />
    </>
  );
}
