"use client";

import { useCallback, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import {
  addFiles,
  addRecipients,
  createShareAlias,
  deleteShare,
  getDownloadUrl,
  notifyRecipients,
  updateShare,
} from "@/http/endpoints";
import type { Share } from "@/http/endpoints/shares/types";

export interface ShareManagerHook {
  shareToDelete: Share | null;
  shareToEdit: Share | null;
  shareToManageFiles: Share | null;
  shareToManageRecipients: Share | null;
  shareToManageSecurity: Share | null;
  shareToManageExpiration: Share | null;
  shareToViewDetails: Share | null;
  shareToGenerateLink: Share | null;
  shareToViewQrCode: Share | null;
  sharesToDelete: Share[] | null;
  setShareToDelete: (share: Share | null) => void;
  setShareToEdit: (share: Share | null) => void;
  setShareToManageFiles: (share: Share | null) => void;
  setShareToManageRecipients: (share: Share | null) => void;
  setShareToManageSecurity: (share: Share | null) => void;
  setShareToManageExpiration: (share: Share | null) => void;
  setShareToViewDetails: (share: Share | null) => void;
  setShareToGenerateLink: (share: Share | null) => void;
  setShareToViewQrCode: (share: Share | null) => void;
  setSharesToDelete: (shares: Share[] | null) => void;
  handleDelete: (shareId: string) => Promise<void>;
  handleBulkDelete: (shares: Share[]) => void;
  handleBulkDownload: (shares: Share[]) => void;
  handleDownloadShareFiles: (share: Share) => Promise<void>;
  handleBulkDownloadWithZip: (shares: Share[], zipName: string) => Promise<void>;
  handleDeleteBulk: () => Promise<void>;
  handleEdit: (shareId: string, data: any) => Promise<void>;
  handleUpdateName: (shareId: string, newName: string) => Promise<void>;
  handleUpdateDescription: (shareId: string, newDescription: string) => Promise<void>;
  handleUpdateSecurity: (share: Share) => Promise<void>;
  handleUpdateExpiration: (share: Share) => Promise<void>;
  handleManageFiles: (shareId: string, files: any[]) => Promise<void>;
  handleManageRecipients: (shareId: string, recipients: any[]) => Promise<void>;
  handleGenerateLink: (shareId: string, alias: string) => Promise<void>;
  handleNotifyRecipients: (share: Share) => Promise<void>;
  setClearSelectionCallback?: (callback: () => void) => void;
}

export function useShareManager(onSuccess: () => void) {
  const t = useTranslations();
  const [shareToDelete, setShareToDelete] = useState<Share | null>(null);
  const [shareToEdit, setShareToEdit] = useState<Share | null>(null);
  const [shareToManageFiles, setShareToManageFiles] = useState<Share | null>(null);
  const [shareToManageRecipients, setShareToManageRecipients] = useState<Share | null>(null);
  const [shareToManageSecurity, setShareToManageSecurity] = useState<Share | null>(null);
  const [shareToManageExpiration, setShareToManageExpiration] = useState<Share | null>(null);
  const [shareToViewDetails, setShareToViewDetails] = useState<Share | null>(null);
  const [shareToGenerateLink, setShareToGenerateLink] = useState<Share | null>(null);
  const [shareToViewQrCode, setShareToViewQrCode] = useState<Share | null>(null);
  const [sharesToDelete, setSharesToDelete] = useState<Share[] | null>(null);
  const [clearSelectionCallback, setClearSelectionCallbackState] = useState<(() => void) | null>(null);

  const setClearSelectionCallback = useCallback((callback: () => void) => {
    setClearSelectionCallbackState(() => callback);
  }, []);

  const handleDelete = async (shareId: string) => {
    try {
      await deleteShare(shareId);
      toast.success(t("shareManager.deleteSuccess"));
      onSuccess();
      setShareToDelete(null);
    } catch {
      toast.error(t("shareManager.deleteError"));
    }
  };

  const handleBulkDelete = (shares: Share[]) => {
    setSharesToDelete(shares);
  };

  const handleDeleteBulk = async () => {
    if (!sharesToDelete) return;

    const loadingToast = toast.loading(t("shareManager.bulkDeleteLoading", { count: sharesToDelete.length }));

    try {
      await Promise.all(sharesToDelete.map((share) => deleteShare(share.id)));
      toast.dismiss(loadingToast);
      toast.success(t("shareManager.bulkDeleteSuccess", { count: sharesToDelete.length }));
      setSharesToDelete(null);
      onSuccess();

      if (clearSelectionCallback) {
        clearSelectionCallback();
      }
    } catch {
      toast.dismiss(loadingToast);
      toast.error(t("shareManager.bulkDeleteError"));
    }
  };

  const handleEdit = async (shareId: string, data: any) => {
    try {
      await updateShare({ id: shareId, ...data });
      toast.success(t("shareManager.updateSuccess"));
      onSuccess();
      setShareToEdit(null);
    } catch {
      toast.error(t("shareManager.updateError"));
    }
  };

  const handleUpdateName = async (shareId: string, newName: string) => {
    try {
      await updateShare({ id: shareId, name: newName });
      await onSuccess();
      toast.success(t("shareManager.updateSuccess"));
    } catch {
      toast.error(t("shareManager.updateError"));
    }
  };

  const handleUpdateDescription = async (shareId: string, newDescription: string) => {
    try {
      await updateShare({ id: shareId, description: newDescription });
      await onSuccess();
      toast.success(t("shareManager.updateSuccess"));
    } catch {
      toast.error(t("shareManager.updateError"));
    }
  };

  const handleUpdateSecurity = async (share: Share) => {
    setShareToManageSecurity(share);
  };

  const handleUpdateExpiration = async (share: Share) => {
    setShareToManageExpiration(share);
  };

  const handleManageFiles = async (shareId: string, files: string[]) => {
    try {
      await addFiles(shareId, { files });
      toast.success(t("shareManager.filesUpdateSuccess"));
      onSuccess();
      setShareToManageFiles(null);
    } catch {
      toast.error(t("shareManager.filesUpdateError"));
    }
  };

  const handleManageRecipients = async (shareId: string, recipients: string[]) => {
    try {
      await addRecipients(shareId, { emails: recipients });
      toast.success(t("shareManager.recipientsUpdateSuccess"));
      onSuccess();
      setShareToManageRecipients(null);
    } catch {
      toast.error(t("shareManager.recipientsUpdateError"));
    }
  };

  const handleGenerateLink = async (shareId: string, alias: string) => {
    try {
      await createShareAlias(shareId, { alias });
      toast.success(t("shareManager.linkGenerateSuccess"));
      onSuccess();
    } catch (error) {
      toast.error(t("shareManager.linkGenerateError"));
      throw error;
    }
  };

  const handleNotifyRecipients = async (share: Share) => {
    const link = `${window.location.origin}/s/${share.alias?.alias}`;
    const loadingToast = toast.loading(t("shareManager.notifyLoading"));

    try {
      await notifyRecipients(share.id, { shareLink: link });
      toast.dismiss(loadingToast);
      toast.success(t("shareManager.notifySuccess"));
    } catch {
      toast.dismiss(loadingToast);
      toast.error(t("shareManager.notifyError"));
    }
  };

  const handleBulkDownloadWithZip = async (shares: Share[], zipName: string) => {
    try {
      toast.promise(
        (async () => {
          const JSZip = (await import("jszip")).default;
          const zip = new JSZip();

          const allFiles: any[] = [];
          shares.forEach((share) => {
            if (share.files) {
              share.files.forEach((file) => {
                allFiles.push({
                  ...file,
                  shareName: share.name || t("shareManager.defaultShareName"),
                });
              });
            }
          });

          const downloadPromises = allFiles.map(async (file) => {
            try {
              const encodedObjectName = encodeURIComponent(file.objectName);
              const downloadResponse = await getDownloadUrl(encodedObjectName);
              const downloadUrl = downloadResponse.data.url;
              const response = await fetch(downloadUrl);

              if (!response.ok) {
                throw new Error(`Failed to download ${file.name}`);
              }

              const blob = await response.blob();
              const fileName = shares.length > 1 ? `${file.shareName}/${file.name}` : file.name;
              zip.file(fileName, blob);
            } catch (error) {
              console.error(`Error downloading file ${file.name}:`, error);
              throw error;
            }
          });

          await Promise.all(downloadPromises);

          const zipBlob = await zip.generateAsync({ type: "blob" });

          const url = URL.createObjectURL(zipBlob);
          const a = document.createElement("a");
          a.href = url;
          a.download = zipName.endsWith(".zip") ? zipName : `${zipName}.zip`;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
          URL.revokeObjectURL(url);

          if (clearSelectionCallback) {
            clearSelectionCallback();
          }
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

  const handleBulkDownload = (shares: Share[]) => {
    const zipName =
      shares.length === 1
        ? t("shareManager.singleShareZipName", { shareName: shares[0].name || t("shareManager.defaultShareName") })
        : t("shareManager.multipleSharesZipName", { count: shares.length });

    handleBulkDownloadWithZip(shares, zipName);
  };

  const handleDownloadShareFiles = async (share: Share) => {
    if (!share.files || share.files.length === 0) {
      toast.error(t("shareManager.noFilesToDownload"));
      return;
    }

    if (share.files.length === 1) {
      const file = share.files[0];
      try {
        const encodedObjectName = encodeURIComponent(file.objectName);
        const response = await getDownloadUrl(encodedObjectName);
        const downloadUrl = response.data.url;

        const link = document.createElement("a");
        link.href = downloadUrl;
        link.download = file.name;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
        toast.success(t("shareManager.downloadSuccess"));
      } catch (error) {
        console.error("Download error:", error);
        toast.error(t("shareManager.downloadError"));
      }
    } else {
      const zipName = t("shareManager.singleShareZipName", {
        shareName: share.name || t("shareManager.defaultShareName"),
      });
      await handleBulkDownloadWithZip([share], zipName);
    }
  };

  return {
    shareToDelete,
    shareToEdit,
    shareToManageFiles,
    shareToManageRecipients,
    shareToManageSecurity,
    shareToManageExpiration,
    shareToViewDetails,
    shareToGenerateLink,
    shareToViewQrCode,
    sharesToDelete,
    setShareToDelete,
    setShareToEdit,
    setShareToManageFiles,
    setShareToManageRecipients,
    setShareToManageSecurity,
    setShareToManageExpiration,
    setShareToViewDetails,
    setShareToGenerateLink,
    setShareToViewQrCode,
    setSharesToDelete,
    handleDelete,
    handleBulkDelete,
    handleDeleteBulk,
    handleEdit,
    handleUpdateName,
    handleUpdateDescription,
    handleUpdateSecurity,
    handleUpdateExpiration,
    handleManageFiles,
    handleManageRecipients,
    handleGenerateLink,
    handleNotifyRecipients,
    handleBulkDownload,
    handleDownloadShareFiles,
    handleBulkDownloadWithZip,
    setClearSelectionCallback,
  };
}
