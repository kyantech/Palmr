import { SharesTable } from "@/components/tables/shares-table";
import { SharesTableContainerProps } from "../types";
import { EmptySharesState } from "./empty-shares-state";

export function SharesTableContainer({ shares, onCopyLink, onCreateShare, shareManager }: SharesTableContainerProps) {
  return shares.length > 0 ? (
    <SharesTable
      shares={shares}
      onCopyLink={onCopyLink}
      onDelete={shareManager.setShareToDelete}
      onBulkDelete={shareManager.handleBulkDelete}
      onBulkDownload={shareManager.handleBulkDownload}
      onDownloadShareFiles={shareManager.handleDownloadShareFiles}
      onEdit={shareManager.setShareToEdit}
      onUpdateName={shareManager.handleUpdateName}
      onUpdateDescription={shareManager.handleUpdateDescription}
      onUpdateSecurity={shareManager.setShareToManageSecurity}
      onUpdateExpiration={shareManager.setShareToManageExpiration}
      onGenerateLink={shareManager.setShareToGenerateLink}
      onManageFiles={shareManager.setShareToManageFiles}
      onManageRecipients={shareManager.setShareToManageRecipients}
      onNotifyRecipients={shareManager.handleNotifyRecipients}
      onViewQrCode={shareManager.setShareToViewQrCode}
      onViewDetails={shareManager.setShareToViewDetails}
      setClearSelectionCallback={shareManager.setClearSelectionCallback}
    />
  ) : (
    <EmptySharesState onCreateShare={onCreateShare} />
  );
}
