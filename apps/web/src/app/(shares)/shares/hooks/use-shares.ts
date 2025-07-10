"use client";

import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { useSecureConfigValue } from "@/hooks/use-secure-configs";
import { listUserShares, notifyRecipients } from "@/http/endpoints";
import { Share } from "@/http/endpoints/shares/types";

export function useShares() {
  const t = useTranslations();
  const [shares, setShares] = useState<Share[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState("");
  const [shareToGenerateLink, setShareToGenerateLink] = useState<Share | null>(null);

  const { value: smtpEnabled } = useSecureConfigValue("smtpEnabled");

  const loadShares = useCallback(async () => {
    try {
      const response = await listUserShares();
      const allShares = response.data.shares || [];
      const sortedShares = [...allShares].sort(
        (a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      setShares(sortedShares);
    } catch {
      toast.error(t("shares.errors.loadFailed"));
    } finally {
      setIsLoading(false);
    }
  }, [t]);

  useEffect(() => {
    loadShares();
  }, [loadShares]);

  const filteredShares = shares.filter(
    (share) => share.name?.toLowerCase().includes(searchQuery.toLowerCase()) ?? false
  );

  const handleCopyLink = (share: Share) => {
    if (!share.alias?.alias) return;

    const link = `${window.location.origin}/s/${share.alias.alias}`;

    navigator.clipboard.writeText(link);
    toast.success(t("shares.messages.linkCopied"));
  };

  const handleNotifyRecipients = async (share: Share) => {
    if (!share.alias?.alias) return;

    const link = `${window.location.origin}/s/${share.alias.alias}`;

    try {
      await notifyRecipients(share.id, { shareLink: link });
      toast.success(t("shares.messages.recipientsNotified"));
    } catch {
      toast.error(t("shares.errors.notifyFailed"));
    }
  };

  return {
    shares,
    isLoading,
    searchQuery,
    shareToGenerateLink,
    filteredShares,
    smtpEnabled: smtpEnabled || "false",
    setSearchQuery,
    setShareToGenerateLink,
    handleCopyLink,
    handleNotifyRecipients,
    loadShares,
  };
}
