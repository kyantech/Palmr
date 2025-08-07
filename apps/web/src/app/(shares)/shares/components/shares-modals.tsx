import { useState } from "react";
import { useTranslations } from "next-intl";

import { CreateShareModal } from "@/components/modals/create-share-modal";
import { DeleteConfirmationModal } from "@/components/modals/delete-confirmation-modal";
import { GenerateShareLinkModal } from "@/components/modals/generate-share-link-modal";
import { QrCodeModal } from "@/components/modals/qr-code-modal";
import { ShareActionsModals } from "@/components/modals/share-actions-modals";
import { ShareDetailsModal } from "@/components/modals/share-details-modal";
import { ShareExpirationModal } from "@/components/modals/share-expiration-modal";
import { ShareMultipleFilesModal } from "@/components/modals/share-multiple-files-modal";
import { ShareSecurityModal } from "@/components/modals/share-security-modal";
import { SharesModalsProps } from "../types";

export function SharesModals({
  isCreateModalOpen,
  onCloseCreateModal,
  shareToViewDetails,
  shareToGenerateLink,
  shareManager,
  fileManager,
  onSuccess,
  onCloseViewDetails,
  onCloseGenerateLink,
}: SharesModalsProps) {
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
      <CreateShareModal isOpen={isCreateModalOpen} onClose={onCloseCreateModal} onSuccess={handleShareSuccess} />

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
        onSuccess={handleShareSuccess}
        onEditFile={fileManager.handleRename}
      />

      <QrCodeModal
        isOpen={!!shareManager.shareToViewQrCode}
        onClose={() => shareManager.setShareToViewQrCode(null)}
        shareLink={getShareLink(shareManager.shareToViewQrCode)}
        shareName={shareManager.shareToViewQrCode?.name || "Share"}
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

      <ShareDetailsModal
        shareId={shareToViewDetails?.id || null}
        onClose={onCloseViewDetails}
        onUpdateName={shareManager.handleUpdateName}
        onUpdateDescription={shareManager.handleUpdateDescription}
        onUpdateSecurity={shareManager.handleUpdateSecurity}
        onUpdateExpiration={shareManager.handleUpdateExpiration}
        onGenerateLink={shareManager.handleGenerateLink}
        onManageFiles={shareManager.setShareToManageFiles}
        refreshTrigger={shareDetailsRefresh}
        onSuccess={handleShareSuccess}
      />

      <GenerateShareLinkModal
        share={shareToGenerateLink}
        shareId={shareToGenerateLink?.id || null}
        onClose={onCloseGenerateLink}
        onGenerate={shareManager.handleGenerateLink}
        onSuccess={handleShareSuccess}
      />

      <ShareSecurityModal
        shareId={shareManager.shareToManageSecurity?.id || null}
        share={shareManager.shareToManageSecurity || null}
        onClose={() => shareManager.setShareToManageSecurity(null)}
        onSuccess={handleShareSuccess}
      />

      <ShareExpirationModal
        shareId={shareManager.shareToManageExpiration?.id || null}
        share={shareManager.shareToManageExpiration || null}
        onClose={() => shareManager.setShareToManageExpiration(null)}
        onSuccess={handleShareSuccess}
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
