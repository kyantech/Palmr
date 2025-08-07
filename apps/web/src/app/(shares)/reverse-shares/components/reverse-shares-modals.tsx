import { QrCodeModal } from "@/components/modals/qr-code-modal";
import type { CreateReverseShareBody, UpdateReverseShareBody } from "@/http/endpoints/reverse-shares/types";
import { ReverseShare } from "../hooks/use-reverse-shares";
import { CreateReverseShareModal } from "./create-reverse-share-modal";
import { DeleteReverseShareModal } from "./delete-reverse-share-modal";
import { EditReverseShareModal } from "./edit-reverse-share-modal";
import { GenerateAliasModal } from "./generate-alias-modal";
import { ReceivedFilesModal } from "./received-files-modal";
import { ReverseShareDetailsModal } from "./reverse-share-details-modal";

interface ReverseSharesModalsProps {
  isCreateModalOpen: boolean;
  onCloseCreateModal: () => void;
  onCreateReverseShare: (data: CreateReverseShareBody) => Promise<any>;
  isCreating: boolean;
  reverseShareToEdit: ReverseShare | null;
  onCloseEditModal: () => void;
  onUpdateReverseShare: (data: UpdateReverseShareBody) => Promise<any>;
  isUpdating: boolean;
  reverseShareToViewDetails: ReverseShare | null;
  reverseShareToGenerateLink: ReverseShare | null;
  reverseShareToDelete: ReverseShare | null;
  reverseShareToViewFiles: ReverseShare | null;
  reverseShareToViewQrCode: ReverseShare | null;
  isDeleting: boolean;
  onCloseViewDetails: () => void;
  onCloseGenerateLink: () => void;
  onCloseDeleteModal: () => void;
  onCloseViewFiles: () => void;
  onCloseViewQrCode: () => void;
  onConfirmDelete: (reverseShare: ReverseShare) => Promise<void>;
  onCreateAlias: (reverseShareId: string, alias: string) => Promise<void>;
  onCopyLink: (reverseShare: ReverseShare) => void;
  onViewQrCode: (reverseShare: ReverseShare) => void;
  onUpdateReverseShareData?: (id: string, data: any) => Promise<any>;
  onUpdatePassword?: (id: string, data: { hasPassword: boolean; password?: string }) => Promise<any>;
  onToggleActive?: (id: string, isActive: boolean) => Promise<any>;
  onRefreshData?: () => Promise<void>;
  refreshReverseShare?: (id: string) => Promise<void>;
}

export function ReverseSharesModals({
  isCreateModalOpen,
  onCloseCreateModal,
  onCreateReverseShare,
  isCreating,
  reverseShareToEdit,
  onCloseEditModal,
  onUpdateReverseShare,
  isUpdating,
  reverseShareToViewDetails,
  reverseShareToGenerateLink,
  reverseShareToDelete,
  reverseShareToViewFiles,
  reverseShareToViewQrCode,
  isDeleting,
  onCloseViewDetails,
  onCloseGenerateLink,
  onCloseDeleteModal,
  onCloseViewFiles,
  onCloseViewQrCode,
  onConfirmDelete,
  onCreateAlias,
  onCopyLink,
  onViewQrCode,
  onUpdateReverseShareData,
  onUpdatePassword,
  onToggleActive,
  onRefreshData,
  refreshReverseShare,
}: ReverseSharesModalsProps) {
  return (
    <>
      <CreateReverseShareModal
        isOpen={isCreateModalOpen}
        onClose={onCloseCreateModal}
        onCreateReverseShare={onCreateReverseShare}
        isCreating={isCreating}
      />

      <EditReverseShareModal
        reverseShare={reverseShareToEdit}
        isOpen={!!reverseShareToEdit}
        onClose={onCloseEditModal}
        onUpdateReverseShare={onUpdateReverseShare}
        isUpdating={isUpdating}
      />

      <GenerateAliasModal
        reverseShare={reverseShareToGenerateLink}
        isOpen={!!reverseShareToGenerateLink}
        onClose={onCloseGenerateLink}
        onCreateAlias={onCreateAlias}
        onCopyLink={onCopyLink}
      />

      <DeleteReverseShareModal
        reverseShare={reverseShareToDelete}
        isDeleting={isDeleting}
        onClose={onCloseDeleteModal}
        onConfirm={onConfirmDelete}
      />

      <ReverseShareDetailsModal
        reverseShare={reverseShareToViewDetails}
        isOpen={!!reverseShareToViewDetails}
        onClose={onCloseViewDetails}
        onUpdateReverseShare={onUpdateReverseShareData}
        onCreateAlias={onCreateAlias}
        onCopyLink={onCopyLink}
        onUpdatePassword={onUpdatePassword}
        onToggleActive={onToggleActive}
        onViewQrCode={onViewQrCode}
      />

      <ReceivedFilesModal
        reverseShare={reverseShareToViewFiles}
        isOpen={!!reverseShareToViewFiles}
        onClose={onCloseViewFiles}
        onRefresh={onRefreshData}
        refreshReverseShare={refreshReverseShare}
      />

      <QrCodeModal
        isOpen={!!reverseShareToViewQrCode}
        onClose={onCloseViewQrCode}
        shareLink={
          reverseShareToViewQrCode?.alias?.alias
            ? `${typeof window !== "undefined" ? window.location.origin : ""}/r/${reverseShareToViewQrCode.alias.alias}`
            : ""
        }
        shareName={reverseShareToViewQrCode?.name || "Reverse Share"}
      />
    </>
  );
}
