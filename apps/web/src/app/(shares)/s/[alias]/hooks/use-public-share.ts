"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getShareByAlias } from "@/http/endpoints";
import type { Share } from "@/http/endpoints/shares/types";
import { bulkDownloadWithQueue, downloadFileWithQueue } from "@/utils/download-queue-utils";

export function usePublicShare() {
  const t = useTranslations();
  const params = useParams();
  const alias = params?.alias as string;
  const [share, setShare] = useState<Share | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [password, setPassword] = useState("");
  const [isPasswordModalOpen, setIsPasswordModalOpen] = useState(false);
  const [isPasswordError, setIsPasswordError] = useState(false);

  const loadShare = useCallback(
    async (sharePassword?: string) => {
      if (!alias) return;

      const handleShareError = (error: any) => {
        if (error.response?.data?.error === "Password required") {
          setIsPasswordModalOpen(true);
          setShare(null);
        } else if (error.response?.data?.error === "Invalid password") {
          setIsPasswordError(true);
          toast.error(t("share.errors.invalidPassword"));
        } else {
          toast.error(t("share.errors.loadFailed"));
        }
      };

      try {
        setIsLoading(true);
        const response = await getShareByAlias(alias, sharePassword ? { password: sharePassword } : undefined);

        setShare(response.data.share);
        setIsPasswordModalOpen(false);
        setIsPasswordError(false);
      } catch (error: any) {
        handleShareError(error);
      } finally {
        setIsLoading(false);
      }
    },
    [alias, t]
  );

  const handlePasswordSubmit = async () => {
    await loadShare(password);
  };

  const handleDownload = async (objectName: string, fileName: string) => {
    try {
      await downloadFileWithQueue(objectName, fileName, {
        onStart: () => toast.success(t("share.messages.downloadStarted")),
        onFail: () => toast.error(t("share.errors.downloadFailed")),
      });
    } catch {
      // Error already handled in downloadFileWithQueue
    }
  };

  const handleBulkDownload = async () => {
    if (!share || !share.files || share.files.length === 0) {
      toast.error(t("shareManager.noFilesToDownload"));
      return;
    }

    try {
      const zipName = `${share.name || t("shareManager.defaultShareName")}.zip`;

      toast.promise(
        bulkDownloadWithQueue(
          share.files.map((file) => ({
            objectName: file.objectName,
            name: file.name,
            isReverseShare: false,
          })),
          zipName
        ),
        {
          loading: t("shareManager.creatingZip"),
          success: t("shareManager.zipDownloadSuccess"),
          error: t("shareManager.zipDownloadError"),
        }
      );
    } catch (error) {
      console.error("Error creating ZIP:", error);
    }
  };

  useEffect(() => {
    loadShare();
  }, [alias, loadShare]);

  return {
    isLoading,
    share,
    password,
    isPasswordModalOpen,
    isPasswordError,
    setPassword,
    handlePasswordSubmit,
    handleDownload,
    handleBulkDownload,
  };
}
