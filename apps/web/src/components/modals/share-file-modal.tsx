"use client";

import { useEffect, useState } from "react";
import { IconCalendar, IconCopy, IconEye, IconLink, IconLock, IconShare } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { addFiles, createShare, createShareAlias } from "@/http/endpoints";
import { customNanoid } from "@/lib/utils";

interface File {
  id: string;
  name: string;
  description?: string;
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface ShareFileModalProps {
  isOpen: boolean;
  file: File | null;
  onClose: () => void;
  onSuccess: () => void;
}

const generateCustomId = () => customNanoid(10, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");

export function ShareFileModal({ isOpen, file, onClose, onSuccess }: ShareFileModalProps) {
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
    if (isOpen && file) {
      setFormData({
        name: `${file.name.split(".")[0]}`,
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
  }, [isOpen, file]);

  const handleCreateShare = async () => {
    if (!file) return;

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

      await addFiles(newShareId, { files: [file.id] });

      toast.success(t("createShare.success"));
      setStep("link");
    } catch {
      toast.error(t("createShare.error"));
    } finally {
      setIsLoading(false);
    }

    setFormData({
      name: "",
      description: "",
      password: "",
      expiresAt: "",
      isPasswordProtected: false,
      maxViews: "",
    });
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

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {step === "create" ? (
              <>
                <IconShare size={20} />
                {t("shareFile.title")}
              </>
            ) : (
              <>
                <IconLink size={20} />
                {t("shareFile.linkTitle")}
              </>
            )}
          </DialogTitle>
        </DialogHeader>

        {step === "create" && (
          <div className="flex flex-col gap-4">
            <div className="space-y-2">
              <Label>{t("shareFile.nameLabel")}</Label>
              <Input
                value={formData.name}
                onChange={(e) => setFormData({ ...formData, name: e.target.value })}
                placeholder={t("shareFile.namePlaceholder")}
              />
            </div>

            <div className="space-y-2">
              <Label>{t("shareFile.descriptionLabel")}</Label>
              <Input
                value={formData.description}
                onChange={(e) => setFormData({ ...formData, description: e.target.value })}
                placeholder={t("shareFile.descriptionPlaceholder")}
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
                <p className="text-sm text-muted-foreground">{t("shareFile.linkReady")}</p>
                <Input readOnly value={generatedLink} />
              </>
            )}
          </div>
        )}

        <DialogFooter>
          {step === "create" && (
            <>
              <Button variant="outline" onClick={handleClose}>
                {t("common.cancel")}
              </Button>
              <Button disabled={isLoading || !formData.name.trim()} onClick={handleCreateShare}>
                {isLoading ? <div className="animate-spin">⠋</div> : t("shareFile.createShare")}
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
              <Button onClick={handleCopyLink}>
                <IconCopy className="h-4 w-4" />
                {t("shareFile.copyLink")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
