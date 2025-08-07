"use client";

import { useState } from "react";
import { IconCalendar, IconEye, IconLock, IconShare } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { createShare } from "@/http/endpoints";

interface CreateShareModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSuccess: () => void;
}

export function CreateShareModal({ isOpen, onClose, onSuccess }: CreateShareModalProps) {
  const t = useTranslations();
  const [formData, setFormData] = useState({
    name: "",
    description: "",
    password: "",
    expiresAt: "",
    isPasswordProtected: false,
    maxViews: "",
  });
  const [isLoading, setIsLoading] = useState(false);

  const handleSubmit = async () => {
    try {
      setIsLoading(true);
      await createShare({
        name: formData.name,
        description: formData.description || undefined,
        password: formData.isPasswordProtected ? formData.password : undefined,
        expiration: formData.expiresAt
          ? (() => {
              const dateValue = formData.expiresAt;
              if (dateValue.length === 10) {
                return new Date(dateValue + "T23:59:59").toISOString();
              }
              return new Date(dateValue).toISOString();
            })()
          : undefined,
        maxViews: formData.maxViews ? parseInt(formData.maxViews) : undefined,
        files: [],
      });
      toast.success(t("createShare.success"));
      onSuccess();
      onClose();
      setFormData({
        name: "",
        description: "",
        password: "",
        expiresAt: "",
        isPasswordProtected: false,
        maxViews: "",
      });
    } catch {
      toast.error(t("createShare.error"));
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <IconShare size={20} />
            {t("createShare.title")}
          </DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <div className="space-y-2">
            <Label>{t("createShare.nameLabel")}</Label>
            <Input
              value={formData.name}
              onChange={(e) => setFormData({ ...formData, name: e.target.value })}
              onPaste={(e) => {
                e.preventDefault();
                const pastedText = e.clipboardData.getData("text");
                setFormData({ ...formData, name: pastedText });
              }}
              placeholder={t("createShare.namePlaceholder")}
            />
          </div>

          <div className="space-y-2">
            <Label>{t("createShare.descriptionLabel")}</Label>
            <Input
              value={formData.description}
              onChange={(e) => setFormData({ ...formData, description: e.target.value })}
              placeholder={t("createShare.descriptionPlaceholder")}
            />
          </div>

          <div className="space-y-2">
            <Label className="flex items-center gap-2">
              <IconCalendar size={16} />
              {t("createShare.expirationLabel")}
            </Label>
            <Input
              placeholder={t("createShare.expirationPlaceholder")}
              type="datetime-local"
              value={formData.expiresAt}
              onChange={(e) => setFormData({ ...formData, expiresAt: e.target.value })}
              onBlur={(e) => {
                const value = e.target.value;
                if (value && value.length === 10) {
                  setFormData({ ...formData, expiresAt: value + "T23:59" });
                }
              }}
            />
          </div>

          <div className="space-y-2">
            <Label className="flex items-center gap-2">
              <IconEye size={16} />
              {t("createShare.maxViewsLabel")}
            </Label>
            <Input
              min="1"
              placeholder={t("createShare.maxViewsPlaceholder")}
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
              {t("createShare.passwordProtection")}
            </Label>
          </div>

          {formData.isPasswordProtected && (
            <div className="space-y-2">
              <Label>{t("createShare.passwordLabel")}</Label>
              <Input
                type="password"
                value={formData.password}
                onChange={(e) => setFormData({ ...formData, password: e.target.value })}
              />
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button disabled={isLoading} onClick={handleSubmit}>
            {isLoading ? <div className="animate-spin">⠋</div> : t("createShare.create")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
