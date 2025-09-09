"use client";

import { useState } from "react";
import { IconDownload, IconX } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface BulkItem {
  id: string;
  name: string;
  size?: number;
  type: "file" | "folder";
}

interface BulkDownloadModalProps {
  isOpen: boolean;
  onClose: () => void;
  onDownload: (zipName: string) => void;
  items?: BulkItem[];
}

export function BulkDownloadModal({ isOpen, onClose, onDownload, items = [] }: BulkDownloadModalProps) {
  const t = useTranslations();
  const [zipName, setZipName] = useState("");

  const handleClose = () => {
    onClose();
    setZipName("");
  };
  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (zipName.trim()) {
      onDownload(zipName.trim());
      handleClose();
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <IconDownload size={20} />
            {t("bulkDownload.title")}
          </DialogTitle>
        </DialogHeader>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="zipName">{t("bulkDownload.zipNameLabel")}</Label>
            <Input
              id="zipName"
              value={zipName}
              onChange={(e) => setZipName(e.target.value)}
              placeholder={t("bulkDownload.zipNamePlaceholder")}
              className="w-full"
              autoFocus
            />
            <p className="text-sm text-muted-foreground">
              {t("bulkDownload.description", { count: items.length })}(
              {items.filter((item) => item.type === "file").length} files,{" "}
              {items.filter((item) => item.type === "folder").length} folders)
            </p>
          </div>
        </form>

        <DialogFooter className="flex gap-2">
          <Button variant="outline" onClick={handleClose}>
            <IconX className="h-4 w-4" />
            {t("common.cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={!zipName.trim()}>
            <IconDownload className="h-4 w-4" />
            {t("bulkDownload.download")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
