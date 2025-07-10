"use client";

import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { useFileManager } from "@/hooks/use-file-manager";
import { useSecureConfigValue } from "@/hooks/use-secure-configs";
import { useShareManager } from "@/hooks/use-share-manager";
import { getDiskSpace, listFiles, listUserShares } from "@/http/endpoints";
import { Share } from "@/http/endpoints/shares/types";

export function useDashboard() {
  const t = useTranslations();
  const [diskSpace, setDiskSpace] = useState<{
    diskSizeGB: number;
    diskUsedGB: number;
    diskAvailableGB: number;
    uploadAllowed: boolean;
  } | null>(null);
  const [diskSpaceError, setDiskSpaceError] = useState<string | null>(null);
  const [recentFiles, setRecentFiles] = useState<any[]>([]);
  const [recentShares, setRecentShares] = useState<any[]>([]);
  const [isLoading, setIsLoading] = useState(true);

  const { value: smtpEnabled } = useSecureConfigValue("smtpEnabled");

  const [isUploadModalOpen, setIsUploadModalOpen] = useState(false);
  const [isCreateModalOpen, setIsCreateModalOpen] = useState(false);

  const onOpenUploadModal = () => setIsUploadModalOpen(true);
  const onCloseUploadModal = () => setIsUploadModalOpen(false);
  const onOpenCreateModal = () => setIsCreateModalOpen(true);
  const onCloseCreateModal = () => setIsCreateModalOpen(false);

  const loadDashboardData = useCallback(async () => {
    try {
      const loadDiskSpace = async () => {
        try {
          const diskSpaceRes = await getDiskSpace();
          setDiskSpace(diskSpaceRes.data);
          setDiskSpaceError(null);
        } catch (error: any) {
          console.warn("Failed to load disk space:", error);
          setDiskSpace(null);

          if (error.response?.status === 503 && error.response?.data?.code === "DISK_SPACE_DETECTION_FAILED") {
            setDiskSpaceError("disk_detection_failed");
          } else if (error.response?.status >= 500) {
            setDiskSpaceError("server_error");
          } else {
            setDiskSpaceError("unknown_error");
          }
        }
      };

      const loadFilesAndShares = async () => {
        const [filesRes, sharesRes] = await Promise.all([listFiles(), listUserShares()]);

        const allFiles = filesRes.data.files || [];
        const sortedFiles = [...allFiles].sort(
          (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
        );
        setRecentFiles(sortedFiles.slice(0, 5));

        const allShares = sharesRes.data.shares || [];
        const sortedShares = [...allShares].sort(
          (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
        );
        setRecentShares(sortedShares.slice(0, 5));
      };

      await Promise.allSettled([loadDiskSpace(), loadFilesAndShares()]);
    } catch (error) {
      console.error("Critical dashboard error:", error);
      toast.error(t("dashboard.loadError"));
    } finally {
      setIsLoading(false);
    }
  }, [t]);

  const fileManager = useFileManager(loadDashboardData);
  const shareManager = useShareManager(loadDashboardData);

  const handleCopyLink = (share: Share) => {
    if (!share.alias?.alias) return;
    const link = `${window.location.origin}/s/${share.alias.alias}`;

    navigator.clipboard.writeText(link);
    toast.success(t("dashboard.linkCopied"));
  };

  useEffect(() => {
    loadDashboardData();
  }, [loadDashboardData]);

  return {
    isLoading,
    diskSpace,
    diskSpaceError,
    recentFiles,
    recentShares,
    modals: {
      isUploadModalOpen,
      isCreateModalOpen,
      onOpenUploadModal,
      onCloseUploadModal,
      onOpenCreateModal,
      onCloseCreateModal,
    },
    fileManager,
    shareManager,
    handleCopyLink,
    loadDashboardData,
    smtpEnabled: smtpEnabled || "false",
  };
}
