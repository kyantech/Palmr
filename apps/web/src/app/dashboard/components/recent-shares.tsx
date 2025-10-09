import { useRouter } from "next/navigation";
import { IconPlus, IconShare } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { SharesTable } from "@/components/tables/shares-table";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { RecentSharesProps } from "../types";
import { EmptySharesState } from "./empty-shares-state";

export function RecentShares({ shares, shareManager, onOpenCreateModal, onCopyLink }: RecentSharesProps) {
  const t = useTranslations();
  const router = useRouter();

  return (
    <Card>
      <CardContent>
        <div className="flex flex-col gap-6">
          <div className="flex flex-col sm:flex-row justify-between sm:items-center gap-4">
            <h2 className="text-xl font-semibold flex items-center gap-2">
              <IconShare className="text-xl text-gray-500" />
              {t("recentShares.title")}
            </h2>

            <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
              <Button
                className="font-semibold text-sm cursor-pointer"
                variant="outline"
                size="default"
                onClick={() => router.push("/shares")}
              >
                <IconShare className="h-4 w-4" />
                {t("recentShares.viewAll")}
              </Button>

              <Button
                className="font-semibold text-sm cursor-pointer"
                variant="outline"
                size="default"
                onClick={onOpenCreateModal}
              >
                <IconPlus className="h-4 w-4" />
                {t("recentShares.createShare")}
              </Button>
            </div>
          </div>

          {shares.length > 0 ? (
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
            <EmptySharesState onCreate={onOpenCreateModal} />
          )}
        </div>
      </CardContent>
    </Card>
  );
}
