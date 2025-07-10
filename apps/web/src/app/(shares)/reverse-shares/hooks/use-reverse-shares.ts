"use client";

import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import {
  createReverseShare,
  createReverseShareAlias,
  deleteReverseShare,
  listUserReverseShares,
  updateReverseShare,
  updateReverseSharePassword,
} from "@/http/endpoints";
import type {
  CreateReverseShareBody,
  ListUserReverseSharesResult,
  UpdateReverseShareBody,
} from "@/http/endpoints/reverse-shares/types";

export type ReverseShare = ListUserReverseSharesResult["data"]["reverseShares"][0];

export function useReverseShares() {
  const t = useTranslations();
  const [reverseShares, setReverseShares] = useState<ReverseShare[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState("");
  const [reverseShareToViewDetails, setReverseShareToViewDetails] = useState<ReverseShare | null>(null);
  const [reverseShareToGenerateLink, setReverseShareToGenerateLink] = useState<ReverseShare | null>(null);
  const [reverseShareToDelete, setReverseShareToDelete] = useState<ReverseShare | null>(null);
  const [reverseShareToEdit, setReverseShareToEdit] = useState<ReverseShare | null>(null);
  const [reverseShareToViewFiles, setReverseShareToViewFiles] = useState<ReverseShare | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);
  const [isCreateModalOpen, setIsCreateModalOpen] = useState(false);
  const [isCreating, setIsCreating] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);

  const loadReverseShares = useCallback(async () => {
    try {
      const response = await listUserReverseShares();
      const allReverseShares = response.data.reverseShares || [];
      const sortedReverseShares = [...allReverseShares].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      setReverseShares(sortedReverseShares);
    } catch {
      toast.error(t("reverseShares.errors.loadFailed"));
    } finally {
      setIsLoading(false);
    }
  }, [t]);

  const refreshReverseShare = async (id: string) => {
    try {
      const response = await listUserReverseShares();
      const allReverseShares = response.data.reverseShares || [];
      const sortedReverseShares = [...allReverseShares].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      setReverseShares(sortedReverseShares);

      const updatedReverseShare = allReverseShares.find((rs) => rs.id === id);
      if (updatedReverseShare) {
        if (reverseShareToViewFiles && reverseShareToViewFiles.id === id) {
          setReverseShareToViewFiles(updatedReverseShare as ReverseShare);
        }
        if (reverseShareToViewDetails && reverseShareToViewDetails.id === id) {
          setReverseShareToViewDetails(updatedReverseShare as ReverseShare);
        }
      }
    } catch {
      toast.error(t("reverseShares.errors.loadFailed"));
    }
  };

  const handleCreateReverseShare = async (data: CreateReverseShareBody) => {
    setIsCreating(true);
    try {
      const response = await createReverseShare(data);
      const newReverseShare = response.data.reverseShare;

      setReverseShares((prev) => [newReverseShare as ReverseShare, ...prev]);

      toast.success(t("reverseShares.messages.createSuccess"));
      setIsCreateModalOpen(false);

      setReverseShareToGenerateLink(newReverseShare as ReverseShare);

      return newReverseShare;
    } catch {
      toast.error(t("reverseShares.errors.createFailed"));
    } finally {
      setIsCreating(false);
    }
  };

  const handleCreateAlias = async (reverseShareId: string, alias: string) => {
    try {
      await createReverseShareAlias(reverseShareId, { alias });

      const newAlias = {
        id: "",
        alias,
        reverseShareId,
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
      };

      setReverseShares((prev) =>
        prev.map((rs) =>
          rs.id === reverseShareId
            ? {
                ...rs,
                alias: newAlias,
              }
            : rs
        )
      );

      if (reverseShareToViewDetails && reverseShareToViewDetails.id === reverseShareId) {
        setReverseShareToViewDetails({
          ...reverseShareToViewDetails,
          alias: newAlias,
        });
      }

      toast.success(t("reverseShares.messages.aliasCreated"));
    } catch {
      toast.error(t("reverseShares.errors.aliasCreateFailed"));
    }
  };

  const handleDeleteReverseShare = async (reverseShare: ReverseShare) => {
    setIsDeleting(true);
    try {
      await deleteReverseShare(reverseShare.id);

      setReverseShares((prev) => prev.filter((rs) => rs.id !== reverseShare.id));

      toast.success(t("reverseShares.messages.deleteSuccess"));
      setReverseShareToDelete(null);
    } catch {
      toast.error(t("reverseShares.errors.deleteFailed"));
    } finally {
      setIsDeleting(false);
    }
  };

  const handleUpdateReverseShare = async (data: UpdateReverseShareBody) => {
    setIsUpdating(true);
    try {
      const response = await updateReverseShare(data);
      const updatedReverseShare = response.data.reverseShare;

      setReverseShares((prev) =>
        prev.map((rs) => (rs.id === data.id ? ({ ...rs, ...updatedReverseShare } as ReverseShare) : rs))
      );

      toast.success(t("reverseShares.messages.updateSuccess"));
      setReverseShareToEdit(null);

      return updatedReverseShare;
    } catch {
      toast.error(t("reverseShares.errors.updateFailed"));
    } finally {
      setIsUpdating(false);
    }
  };

  const handleUpdatePassword = async (id: string, data: { hasPassword: boolean; password?: string }) => {
    try {
      const payload = { password: data.hasPassword ? data.password! : null };
      const response = await updateReverseSharePassword(id, payload);
      const updatedReverseShare = response.data.reverseShare;

      setReverseShares((prev) =>
        prev.map((rs) => (rs.id === id ? ({ ...rs, ...updatedReverseShare } as ReverseShare) : rs))
      );

      if (reverseShareToViewDetails && reverseShareToViewDetails.id === id) {
        setReverseShareToViewDetails({ ...reverseShareToViewDetails, ...updatedReverseShare } as ReverseShare);
      }

      return updatedReverseShare;
    } catch {
      toast.error(t("reverseShares.errors.updateFailed"));
    }
  };

  const handleUpdateReverseShareData = async (id: string, data: any) => {
    try {
      const payload: UpdateReverseShareBody = { id, ...data };
      const response = await updateReverseShare(payload);
      const updatedReverseShare = response.data.reverseShare;

      setReverseShares((prev) =>
        prev.map((rs) => (rs.id === id ? ({ ...rs, ...updatedReverseShare } as ReverseShare) : rs))
      );

      if (reverseShareToViewDetails && reverseShareToViewDetails.id === id) {
        setReverseShareToViewDetails({ ...reverseShareToViewDetails, ...updatedReverseShare } as ReverseShare);
      }

      toast.success(t("reverseShares.messages.updateSuccess"));
      return updatedReverseShare;
    } catch {
      toast.error(t("reverseShares.errors.updateFailed"));
    }
  };

  const handleToggleActive = async (id: string, isActive: boolean) => {
    try {
      const payload: UpdateReverseShareBody = { id, isActive };
      const response = await updateReverseShare(payload);
      const updatedReverseShare = response.data.reverseShare;

      setReverseShares((prev) =>
        prev.map((rs) => (rs.id === id ? ({ ...rs, ...updatedReverseShare } as ReverseShare) : rs))
      );

      if (reverseShareToViewDetails && reverseShareToViewDetails.id === id) {
        setReverseShareToViewDetails({ ...reverseShareToViewDetails, ...updatedReverseShare } as ReverseShare);
      }

      toast.success(
        isActive ? t("reverseShares.messages.activateSuccess") : t("reverseShares.messages.deactivateSuccess")
      );
      return updatedReverseShare;
    } catch {
      toast.error(t("reverseShares.errors.updateFailed"));
    }
  };

  useEffect(() => {
    loadReverseShares();
  }, [loadReverseShares]);

  useEffect(() => {
    if (reverseShareToViewDetails) {
      const updatedReverseShare = reverseShares.find((rs) => rs.id === reverseShareToViewDetails.id);
      if (updatedReverseShare) {
        setReverseShareToViewDetails(updatedReverseShare);
      }
    }
  }, [reverseShares, reverseShareToViewDetails]);

  useEffect(() => {
    if (reverseShareToViewFiles) {
      const updatedReverseShare = reverseShares.find((rs) => rs.id === reverseShareToViewFiles.id);
      if (updatedReverseShare) {
        setReverseShareToViewFiles(updatedReverseShare);
      }
    }
  }, [reverseShares, reverseShareToViewFiles]);

  const filteredReverseShares = reverseShares.filter(
    (reverseShare) => reverseShare.name?.toLowerCase().includes(searchQuery.toLowerCase()) ?? false
  );

  const handleCopyLink = (reverseShare: ReverseShare) => {
    if (!reverseShare.alias?.alias) return;

    const link = `${window.location.origin}/r/${reverseShare.alias.alias}`;

    navigator.clipboard.writeText(link);
    toast.success(t("reverseShares.messages.linkCopied"));
  };

  return {
    reverseShares,
    isLoading,
    searchQuery,
    reverseShareToViewDetails,
    reverseShareToGenerateLink,
    reverseShareToDelete,
    reverseShareToEdit,
    reverseShareToViewFiles,
    isDeleting,
    isCreateModalOpen,
    isCreating,
    isUpdating,
    filteredReverseShares,
    setSearchQuery,
    setReverseShareToViewDetails,
    setReverseShareToGenerateLink,
    setReverseShareToDelete,
    setReverseShareToEdit,
    setReverseShareToViewFiles,
    setIsCreateModalOpen,
    handleCopyLink,
    handleDeleteReverseShare,
    handleCreateReverseShare,
    handleUpdateReverseShare,
    handleCreateAlias,
    handleUpdatePassword,
    handleUpdateReverseShareData,
    handleToggleActive,
    loadReverseShares,
    refreshReverseShare,
  };
}
