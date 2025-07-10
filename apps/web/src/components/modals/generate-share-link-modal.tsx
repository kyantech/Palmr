"use client";

import { useEffect, useState } from "react";
import { IconCopy } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Share } from "@/http/endpoints/shares/types";
import { customNanoid } from "@/lib/utils";

interface GenerateShareLinkModalProps {
  shareId: string | null;
  share: Share | null;
  onClose: () => void;
  onSuccess: () => void;
  onGenerate: (shareId: string, alias: string) => Promise<void>;
}

const generateCustomId = () => customNanoid(10, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");

export function GenerateShareLinkModal({
  shareId,
  share,
  onClose,
  onSuccess,
  onGenerate,
}: GenerateShareLinkModalProps) {
  const t = useTranslations();
  const [alias, setAlias] = useState(() => generateCustomId());
  const [isLoading, setIsLoading] = useState(false);
  const [generatedLink, setGeneratedLink] = useState("");
  const [isEdit, setIsEdit] = useState(false);

  useEffect(() => {
    if (shareId && share?.alias?.alias) {
      setIsEdit(true);
      setAlias(share.alias.alias);
    } else {
      setIsEdit(false);
      setAlias(generateCustomId());
    }
    setGeneratedLink("");
  }, [shareId, share]);

  const handleGenerate = async () => {
    if (!shareId) return;

    try {
      setIsLoading(true);
      await onGenerate(shareId, alias);
      const link = `${window.location.origin}/s/${alias}`;

      setGeneratedLink(link);
      onSuccess();
      toast.success(t("generateShareLink.success"));
    } catch {
      toast.error(t("generateShareLink.error"));
    } finally {
      setIsLoading(false);
    }
  };

  const handleCopyLink = () => {
    navigator.clipboard.writeText(generatedLink);
    toast.success(t("generateShareLink.copied"));
  };

  return (
    <Dialog open={!!shareId} onOpenChange={() => onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            {isEdit ? t("generateShareLink.updateTitle") : t("generateShareLink.generateTitle")}
          </DialogTitle>
        </DialogHeader>
        {!generatedLink ? (
          <div className="space-y-4">
            <p className="text-sm text-muted-foreground">
              {isEdit ? t("generateShareLink.updateDescription") : t("generateShareLink.generateDescription")}
            </p>
            <Input
              placeholder={t("generateShareLink.aliasPlaceholder")}
              value={alias}
              onChange={(e) => setAlias(e.target.value)}
            />
          </div>
        ) : (
          <div className="space-y-4">
            <p className="text-sm text-muted-foreground">{t("generateShareLink.linkReady")}</p>
            <Input readOnly value={generatedLink} />
          </div>
        )}
        <DialogFooter>
          {!generatedLink ? (
            <Button disabled={!alias || isLoading} onClick={handleGenerate}>
              {isEdit ? t("generateShareLink.updateButton") : t("generateShareLink.generateButton")}
            </Button>
          ) : (
            <Button onClick={handleCopyLink}>
              <IconCopy className="h-4 w-4" />
              {t("generateShareLink.copyButton")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
