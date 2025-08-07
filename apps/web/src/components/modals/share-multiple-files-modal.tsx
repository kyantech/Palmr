"use client";

import { useEffect, useState } from "react";
import { IconCalendar, IconCopy, IconDownload, IconEye, IconLink, IconLock, IconShare } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import QRCode from "react-qr-code";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { addFiles, createShare, createShareAlias } from "@/http/endpoints";
import { customNanoid } from "@/lib/utils";

interface BulkFile {
  id: string;
  name: string;
  description?: string;
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface ShareMultipleFilesModalProps {
  files: BulkFile[] | null;
  isOpen: boolean;
  onClose: () => void;
  onSuccess: () => void;
}

const generateCustomId = () => customNanoid(10, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");

export function ShareMultipleFilesModal({ files, isOpen, onClose, onSuccess }: ShareMultipleFilesModalProps) {
  const t = useTranslations();
  const [step, setStep] = useState<"create" | "link">("create");
  const [shareId, setShareId] = useState<string | null>(null);
  const [formData, setFormData] = useState({
    name: "",
    description: "",
    password: "",
    expiresAt: "",
    isPasswordProtected: false,
    maxViews: "",
  });
  const [alias, setAlias] = useState(() => generateCustomId());
  const [generatedLink, setGeneratedLink] = useState("");
  const [isLoading, setIsLoading] = useState(false);

  useEffect(() => {
    if (isOpen && files && files.length > 0) {
      setFormData({
        name: files.length === 1 ? `${files[0].name.split(".")[0]}` : `${files.length} arquivos compartilhados`,
        description: "",
        password: "",
        expiresAt: "",
        isPasswordProtected: false,
        maxViews: "",
      });
      setAlias(generateCustomId());
      setStep("create");
      setShareId(null);
      setGeneratedLink("");
    }
  }, [isOpen, files]);

  const handleCreateShare = async () => {
    if (!files || files.length === 0) return;

    try {
      setIsLoading(true);
      const shareResponse = await createShare({
        name: formData.name,
        description: formData.description || undefined,
        password: formData.isPasswordProtected ? formData.password : undefined,
        expiration: formData.expiresAt ? new Date(formData.expiresAt).toISOString() : undefined,
        maxViews: formData.maxViews ? parseInt(formData.maxViews) : undefined,
        files: [],
      });

      const newShareId = shareResponse.data.share.id;
      setShareId(newShareId);

      await addFiles(newShareId, { files: files.map((f) => f.id) });

      toast.success(t("createShare.success"));
      setStep("link");
    } catch {
      toast.error(t("createShare.error"));
    } finally {
      setIsLoading(false);
    }
  };

  const handleGenerateLink = async () => {
    if (!shareId) return;

    try {
      setIsLoading(true);
      await createShareAlias(shareId, { alias });
      const link = `${window.location.origin}/s/${alias}`;
      setGeneratedLink(link);
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
    const qrCodeElement = document.getElementById("share-multiple-files-qr-code");
    if (qrCodeElement) {
      const canvas = qrCodeElement.querySelector("canvas");
      if (canvas) {
        const link = document.createElement("a");
        link.download = "share-multiple-files-qr-code.png";
        link.href = canvas.toDataURL("image/png");
        link.click();
      }
    }
  };

  const handleClose = () => {
    onClose();
    setTimeout(() => {
      setStep("create");
      setShareId(null);
      setGeneratedLink("");
      setFormData({
        name: "",
        description: "",
        password: "",
        expiresAt: "",
        isPasswordProtected: false,
        maxViews: "",
      });
    }, 300);
  };

  const handleSuccess = () => {
    onSuccess();
    handleClose();
  };

  if (!files) return null;

  const totalSize = files.reduce((sum, file) => sum + file.size, 0);
  const formatFileSize = (bytes: number) => {
    const sizes = ["B", "KB", "MB", "GB"];
    if (bytes === 0) return "0 B";
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return `${Math.round((bytes / Math.pow(1024, i)) * 100) / 100} ${sizes[i]}`;
  };

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-md max-h-[90vh] overflow-hidden flex flex-col">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {step === "create" ? (
              <>
                <IconShare size={20} />
                {t("shareMultipleFiles.title")}
              </>
            ) : (
              <>
                <IconLink size={20} />
                {t("shareFile.linkTitle")}
              </>
            )}
          </DialogTitle>
        </DialogHeader>

        <div className="flex-1 overflow-auto">
          {step === "create" && (
            <div className="space-y-4">
              <div className="space-y-2">
                <Label>{t("shareMultipleFiles.shareNameLabel")} *</Label>
                <Input
                  value={formData.name}
                  onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                  placeholder={t("shareMultipleFiles.shareNamePlaceholder")}
                  required
                />
              </div>

              <div className="space-y-2">
                <Label>{t("shareMultipleFiles.descriptionLabel")}</Label>
                <Input
                  value={formData.description}
                  onChange={(e) => setFormData({ ...formData, description: e.target.value })}
                  placeholder={t("shareMultipleFiles.descriptionPlaceholder")}
                />
              </div>

              <div className="space-y-2">
                <Label className="flex items-center gap-2">
                  <IconCalendar size={16} />
                  {t("shareFile.expirationLabel")}
                </Label>
                <Input
                  placeholder={t("shareFile.expirationPlaceholder")}
                  type="datetime-local"
                  value={formData.expiresAt}
                  onChange={(e) => setFormData({ ...formData, expiresAt: e.target.value })}
                />
              </div>

              <div className="space-y-2">
                <Label className="flex items-center gap-2">
                  <IconEye size={16} />
                  {t("shareFile.maxViewsLabel")}
                </Label>
                <Input
                  min="1"
                  placeholder={t("shareFile.maxViewsPlaceholder")}
                  type="number"
                  value={formData.maxViews}
                  onChange={(e) => setFormData({ ...formData, maxViews: e.target.value })}
                />
              </div>

              <div className="flex items-center gap-2">
                <Switch
                  checked={formData.isPasswordProtected}
                  onCheckedChange={(checked) =>
                    setFormData({
                      ...formData,
                      isPasswordProtected: checked,
                      password: "",
                    })
                  }
                  id="password-protection"
                />
                <Label htmlFor="password-protection" className="flex items-center gap-2">
                  <IconLock size={16} />
                  {t("shareFile.passwordProtection")}
                </Label>
              </div>

              {formData.isPasswordProtected && (
                <div className="space-y-2">
                  <Label>{t("shareFile.passwordLabel")}</Label>
                  <Input
                    type="password"
                    value={formData.password}
                    onChange={(e) => setFormData({ ...formData, password: e.target.value })}
                    placeholder={t("shareFile.passwordPlaceholder")}
                  />
                </div>
              )}

              <div className="space-y-2">
                <Label>
                  {t("shareMultipleFiles.filesToShare")} ({files.length} {t("shareMultipleFiles.files")})
                </Label>
                <ScrollArea className="h-32 w-full rounded-md border p-2 bg-muted/30">
                  <div className="space-y-1">
                    {files.map((file) => (
                      <div key={file.id} className="flex justify-between items-center text-sm">
                        <span className="truncate flex-1">{file.name}</span>
                        <span className="text-muted-foreground ml-2">{formatFileSize(file.size)}</span>
                      </div>
                    ))}
                  </div>
                </ScrollArea>
                <p className="text-xs text-muted-foreground">
                  {t("shareMultipleFiles.totalSize")}: {formatFileSize(totalSize)}
                </p>
              </div>
            </div>
          )}

          {step === "link" && (
            <div className="space-y-4">
              {!generatedLink ? (
                <>
                  <p className="text-sm text-muted-foreground">{t("shareFile.linkDescription")}</p>
                  <div className="space-y-2">
                    <Label>{t("shareFile.aliasLabel")}</Label>
                    <Input
                      placeholder={t("shareFile.aliasPlaceholder")}
                      value={alias}
                      onChange={(e) => setAlias(e.target.value)}
                    />
                  </div>
                </>
              ) : (
                <>
                  <div className="flex flex-col items-center justify-center">
                    <div className="p-4 bg-white rounded-lg">
                      <svg style={{ display: "none" }} /> {/* For SSR safety */}
                      <QRCode
                        id="share-multiple-files-qr-code"
                        value={generatedLink}
                        size={250}
                        level="H"
                        fgColor="#000000"
                        bgColor="#FFFFFF"
                      />
                    </div>
                  </div>
                  <p className="text-sm text-muted-foreground">{t("shareFile.linkReady")}</p>
                  <div className="flex gap-2">
                    <Input readOnly value={generatedLink} className="flex-1" />
                    <Button variant="outline" size="icon" onClick={handleCopyLink} title={t("shareFile.copyLink")}>
                      <IconCopy className="h-4 w-4" />
                    </Button>
                  </div>
                </>
              )}
            </div>
          )}
        </div>

        <DialogFooter>
          {step === "create" && (
            <>
              <Button variant="outline" onClick={handleClose}>
                {t("common.cancel")}
              </Button>
              <Button
                disabled={
                  isLoading || !formData.name.trim() || (formData.isPasswordProtected && !formData.password.trim())
                }
                onClick={handleCreateShare}
              >
                {isLoading ? <div className="animate-spin">⠋</div> : t("shareMultipleFiles.create")}
              </Button>
            </>
          )}

          {step === "link" && !generatedLink && (
            <>
              <Button variant="outline" onClick={() => setStep("create")}>
                {t("common.back")}
              </Button>
              <Button disabled={!alias || isLoading} onClick={handleGenerateLink}>
                {isLoading ? <div className="animate-spin">⠋</div> : t("shareFile.generateLink")}
              </Button>
            </>
          )}

          {step === "link" && generatedLink && (
            <>
              <Button variant="outline" onClick={handleSuccess}>
                {t("common.close")}
              </Button>
              <Button onClick={downloadQRCode}>
                <IconDownload className="h-4 w-4" />
                {t("qrCodeModal.download")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
