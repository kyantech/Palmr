"use client";

import { useEffect, useState } from "react";
import { IconCopy, IconDownload } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import QRCode from "react-qr-code";
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
  const [isDownloading, setIsDownloading] = useState(false);

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

  const downloadQRCode = () => {
    setIsDownloading(true);

    // Get the SVG element
    const svg = document.getElementById("share-link-qr-code");
    if (!svg) {
      setIsDownloading(false);
      return;
    }

    // Create a canvas
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");

    // Set dimensions (with some padding)
    const padding = 20;
    canvas.width = 256 + padding * 2;
    canvas.height = 256 + padding * 2;

    // Fill white background
    if (ctx) {
      ctx.fillStyle = "#FFFFFF";
      ctx.fillRect(0, 0, canvas.width, canvas.height);

      // Convert SVG to data URL
      const svgData = new XMLSerializer().serializeToString(svg);
      const img = new Image();

      img.onload = () => {
        // Draw the image in the center of the canvas with padding
        ctx.drawImage(img, padding, padding, 256, 256);

        // Create a download link
        const link = document.createElement("a");
        link.download = `${share?.name?.replace(/[^a-z0-9]/gi, "-").toLowerCase() || "share"}-qr-code.png`;
        link.href = canvas.toDataURL("image/png");
        link.click();

        setIsDownloading(false);
      };

      img.src = `data:image/svg+xml;base64,${btoa(svgData)}`;
    } else {
      setIsDownloading(false);
    }
  };

  return (
    <Dialog open={!!shareId} onOpenChange={() => onClose()}>
      <DialogContent className="sm:max-w-md">
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
          <div className="space-y-6">
            <p className="text-sm text-muted-foreground text-left">
              {t("generateShareLink.readyDescription", {
                defaultValue:
                  "Your share link is ready. You can scan the QR code directly, download it for later use, or copy the link below.",
              })}
            </p>
            <div className="flex flex-col items-center justify-center">
              <div className="p-4 bg-white rounded-lg">
                <QRCode
                  id="share-link-qr-code"
                  value={generatedLink}
                  size={200}
                  level="H"
                  fgColor="#000000"
                  bgColor="#FFFFFF"
                />
              </div>
            </div>

            <div className="space-y-2">
              <div className="flex space-x-2">
                <Input readOnly value={generatedLink} className="flex-1" />
                <Button
                  variant="outline"
                  size="icon"
                  onClick={handleCopyLink}
                  title={t("generateShareLink.copyButton")}
                >
                  <IconCopy className="h-4 w-4" />
                </Button>
              </div>
            </div>

            <DialogFooter>
              <Button onClick={downloadQRCode} disabled={isDownloading}>
                <IconDownload className="h-4 w-4" />
                {t("qrCodeModal.download")}
              </Button>
            </DialogFooter>
          </div>
        )}
        {!generatedLink && (
          <DialogFooter>
            <Button disabled={!alias || isLoading} onClick={handleGenerate}>
              {isEdit ? t("generateShareLink.updateButton") : t("generateShareLink.generateButton")}
            </Button>
          </DialogFooter>
        )}
      </DialogContent>
    </Dialog>
  );
}
