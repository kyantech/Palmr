"use client";

import { IconLayoutDashboardFilled } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { ProtectedRoute } from "@/components/auth/protected-route";
import { GlobalDropZone } from "@/components/general/global-drop-zone";
import { FileManagerLayout } from "@/components/layout/file-manager-layout";
import { LoadingScreen } from "@/components/layout/loading-screen";
import { QuickAccessCards } from "./components/quick-access-cards";
import { RecentFiles } from "./components/recent-files";
import { RecentShares } from "./components/recent-shares";
import { StorageUsage } from "./components/storage-usage";
import { useDashboard } from "./hooks/use-dashboard";
import { DashboardModals } from "./modals/dashboard-modals";

export default function DashboardPage() {
  const t = useTranslations();

  const {
    isLoading,
    diskSpace,
    diskSpaceError,
    recentFiles,
    recentShares,
    modals,
    fileManager,
    shareManager,
    handleCopyLink,
    loadDashboardData,
  } = useDashboard();

  if (isLoading) {
    return <LoadingScreen />;
  }

  const handleRetryDiskSpace = async () => {
    await loadDashboardData();
  };

  return (
    <ProtectedRoute>
      <GlobalDropZone onSuccess={loadDashboardData}>
        <FileManagerLayout
          breadcrumbLabel={t("dashboard.breadcrumb")}
          icon={<IconLayoutDashboardFilled className="text-xl" />}
          showBreadcrumb={false}
          title={t("dashboard.pageTitle")}
          pendingDownloads={fileManager.pendingDownloads}
          onCancelDownload={fileManager.cancelPendingDownload}
        >
          <StorageUsage diskSpace={diskSpace} diskSpaceError={diskSpaceError} onRetry={handleRetryDiskSpace} />
          <QuickAccessCards />

          <div className="flex flex-col gap-6">
            <RecentFiles
              fileManager={fileManager}
              files={recentFiles}
              isUploadModalOpen={modals.isUploadModalOpen}
              onOpenUploadModal={modals.onOpenUploadModal}
            />

            <RecentShares
              isCreateModalOpen={modals.isCreateModalOpen}
              shareManager={shareManager}
              shares={recentShares}
              onCopyLink={handleCopyLink}
              onOpenCreateModal={modals.onOpenCreateModal}
            />
          </div>

          <DashboardModals
            fileManager={fileManager}
            modals={modals}
            shareManager={shareManager}
            onSuccess={loadDashboardData}
          />
        </FileManagerLayout>
      </GlobalDropZone>
    </ProtectedRoute>
  );
}
