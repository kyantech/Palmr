import { useState } from "react";
import { IconDownload } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import QRCode from "react-qr-code";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

interface QrCodeModalProps {
  isOpen: boolean;
  onClose: () => void;
  shareLink: string;
  shareName: string;
}

export function QrCodeModal({ isOpen, onClose, shareLink, shareName }: QrCodeModalProps) {
  const t = useTranslations();
  const [isDownloading, setIsDownloading] = useState(false);

  const downloadQRCode = () => {
    setIsDownloading(true);

    // Get the SVG element
    const svg = document.getElementById("share-qr-code");
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
        link.download = `${shareName.replace(/[^a-z0-9]/gi, "-").toLowerCase()}-qr-code.png`;
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
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("qrCodeModal.title", { defaultValue: "Share QR Code" })}</DialogTitle>
          <DialogDescription>
            {t("qrCodeModal.description", { defaultValue: "Scan this QR code to access the shared files." })}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col items-center justify-center">
          <div className="p-4 bg-white rounded-lg">
            <QRCode id="share-qr-code" value={shareLink} size={256} level="H" fgColor="#000000" bgColor="#FFFFFF" />
          </div>
          <p className="mt-4 text-sm text-muted-foreground text-center max-w-full break-all">{shareLink}</p>
        </div>

        <DialogFooter className="sm:justify-between flex-row">
          <Button variant="outline" onClick={onClose} className="mt-2 sm:mt-0">
            {t("common.close")}
          </Button>
          <Button onClick={downloadQRCode} className="mt-2 sm:mt-0" disabled={isDownloading}>
            <IconDownload className="h-4 w-4" />
            {t("qrCodeModal.download", { defaultValue: "Download QR Code" })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
