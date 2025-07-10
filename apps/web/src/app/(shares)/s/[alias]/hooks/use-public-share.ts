"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getDownloadUrl, getShareByAlias } from "@/http/endpoints";
import type { Share } from "@/http/endpoints/shares/types";

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
      const encodedObjectName = encodeURIComponent(objectName);
      const response = await getDownloadUrl(encodedObjectName);
      const downloadUrl = response.data.url;

      const link = document.createElement("a");
      link.href = downloadUrl;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      toast.success(t("share.messages.downloadStarted"));
    } catch {
      toast.error(t("share.errors.downloadFailed"));
    }
  };

  const handleBulkDownload = async () => {
    if (!share || !share.files || share.files.length === 0) {
      toast.error(t("shareManager.noFilesToDownload"));
      return;
    }

    try {
      toast.promise(
        (async () => {
          const JSZip = (await import("jszip")).default;
          const zip = new JSZip();

          const downloadPromises = share.files.map(async (file) => {
            try {
              const encodedObjectName = encodeURIComponent(file.objectName);
              const downloadResponse = await getDownloadUrl(encodedObjectName);
              const downloadUrl = downloadResponse.data.url;
              const response = await fetch(downloadUrl);

              if (!response.ok) {
                throw new Error(`Failed to download ${file.name}`);
              }

              const blob = await response.blob();
              zip.file(file.name, blob);
            } catch (error) {
              console.error(`Error downloading file ${file.name}:`, error);
              throw error;
            }
          });

          await Promise.all(downloadPromises);

          const zipBlob = await zip.generateAsync({ type: "blob" });
          const zipName = `${share.name || t("shareManager.defaultShareName")}.zip`;

          const url = URL.createObjectURL(zipBlob);
          const a = document.createElement("a");
          a.href = url;
          a.download = zipName;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
          URL.revokeObjectURL(url);
        })(),
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
