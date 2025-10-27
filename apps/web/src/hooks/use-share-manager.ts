"use client";

import { useCallback, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { addRecipients, createShareAlias, deleteShare, notifyRecipients, updateShare } from "@/http/endpoints";
import { updateFolder } from "@/http/endpoints/folders";
import type { Share } from "@/http/endpoints/shares/types";
import { getCachedDownloadUrl } from "@/lib/download-url-cache";

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
  handleManageFiles: () => Promise<void>;
  handleManageRecipients: (shareId: string, recipients: any[]) => Promise<void>;
  handleGenerateLink: (shareId: string, alias: string) => Promise<void>;
  handleNotifyRecipients: (share: Share) => Promise<void>;
  setClearSelectionCallback?: (callback: () => void) => void;
  handleEditFolder: (folderId: string, newName: string, description?: string) => Promise<void>;
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

  const handleManageFiles = async () => {
    try {
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
      if (shares.length === 1) {
        const share = shares[0];

        const allItems: Array<{
          objectName?: string;
          name: string;
          id?: string;
          type?: "file" | "folder";
        }> = [];

        if (share.files) {
          share.files.forEach((file) => {
            if (!file.folderId) {
              allItems.push({
                objectName: file.objectName,
                name: file.name,
                type: "file",
              });
            }
          });
        }

        if (share.folders) {
          const folderIds = new Set(share.folders.map((f) => f.id));
          share.folders.forEach((folder) => {
            if (!folder.parentId || !folderIds.has(folder.parentId)) {
              allItems.push({
                id: folder.id,
                name: folder.name,
                type: "folder",
              });
            }
          });
        }

        if (allItems.length === 0) {
          toast.error(t("shareManager.noFilesToDownload"));
          return;
        }

        const loadingToast = toast.loading(t("shareManager.preparingDownload"));

        try {
          // Get presigned URLs for all files
          const downloadItems = await Promise.all(
            allItems
              .filter((item) => item.type === "file" && item.objectName)
              .map(async (item) => {
                const url = await getCachedDownloadUrl(item.objectName!);
                return {
                  url,
                  name: item.name,
                };
              })
          );

          if (downloadItems.length === 0) {
            toast.dismiss(loadingToast);
            toast.error(t("shareManager.noFilesToDownload"));
            return;
          }

          // Create ZIP with all files
          const { downloadFilesAsZip } = await import("@/utils/zip-download");
          await downloadFilesAsZip(downloadItems, zipName.endsWith(".zip") ? zipName : `${zipName}.zip`);

          toast.dismiss(loadingToast);
          toast.success(t("shareManager.zipDownloadSuccess"));

          if (clearSelectionCallback) {
            clearSelectionCallback();
          }
        } catch (error) {
          toast.dismiss(loadingToast);
          toast.error(t("shareManager.zipDownloadError"));
          throw error;
        }
      } else {
        toast.error("Multiple share download not yet supported - please download shares individually");
      }
    } catch (error) {
      console.error("Error creating ZIP:", error);
    }
  };

  const handleBulkDownload = (shares: Share[]) => {
    const zipName =
      shares.length === 1
        ? `${shares[0].name || t("shareManager.defaultShareName")}.zip`
        : t("shareManager.multipleSharesZipName", { count: shares.length });
    handleBulkDownloadWithZip(shares, zipName);
  };

  const handleDownloadShareFiles = async (share: Share) => {
    const totalFiles = share.files?.length || 0;
    const totalFolders = share.folders?.length || 0;

    if (totalFiles === 0 && totalFolders === 0) {
      toast.error(t("shareManager.noFilesToDownload"));
      return;
    }

    if (totalFiles === 1 && totalFolders === 0) {
      const file = share.files[0];
      try {
        const loadingToast = toast.loading(t("shareManager.downloading"));
        const url = await getCachedDownloadUrl(file.objectName);

        const link = document.createElement("a");
        link.href = url;
        link.download = file.name;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);

        toast.dismiss(loadingToast);
        toast.success(t("shareManager.downloadSuccess"));
      } catch (error) {
        console.error("Download error:", error);
        toast.error(t("shareManager.downloadError"));
      }
    } else {
      const zipName = `${share.name || t("shareManager.defaultShareName")}.zip`;
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
    handleEditFolder: async (folderId: string, newName: string, description?: string) => {
      try {
        await updateFolder(folderId, { name: newName, description });
        toast.success(t("shareManager.updateSuccess"));
        onSuccess();
      } catch {
        toast.error(t("shareManager.updateError"));
      }
    },
  };
}
