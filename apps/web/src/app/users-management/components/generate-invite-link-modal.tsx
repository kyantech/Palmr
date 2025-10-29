"use client";

import { useState } from "react";
import { IconCheck, IconCopy, IconLink } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { generateInviteToken } from "@/http/endpoints/invite";

interface GenerateInviteLinkModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export function GenerateInviteLinkModal({ isOpen, onClose }: GenerateInviteLinkModalProps) {
  const t = useTranslations();
  const [isGenerating, setIsGenerating] = useState(false);
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const handleGenerate = async () => {
    setIsGenerating(true);
    try {
      const response = await generateInviteToken();

      const inviteUrl = `${window.location.origin}/register-with-invite/${response.token}`;

      setInviteUrl(inviteUrl);
      toast.success(t("users.invite.generated"));
    } catch (error) {
      console.error("Failed to generate invite token:", error);
      toast.error(t("users.invite.errors.generateFailed"));
    } finally {
      setIsGenerating(false);
    }
  };

  const handleCopy = async () => {
    if (!inviteUrl) return;

    try {
      await navigator.clipboard.writeText(inviteUrl);
      setCopied(true);
      toast.success(t("users.invite.linkCopied"));
      setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      console.error("Failed to copy:", error);
      toast.error("Failed to copy link");
    }
  };

  const handleClose = () => {
    setInviteUrl(null);
    setCopied(false);
    onClose();
  };

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-[600px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <IconLink size={24} />
            {t("users.invite.title")}
          </DialogTitle>
          <DialogDescription>{t("users.invite.description")}</DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {!inviteUrl ? (
            <Button onClick={handleGenerate} disabled={isGenerating} className="w-full">
              {isGenerating ? t("users.invite.generating") : t("users.invite.generate")}
            </Button>
          ) : (
            <div className="space-y-4">
              <div className="rounded-lg border bg-muted/50 p-4">
                <h4 className="mb-2 font-semibold text-sm">{t("users.invite.linkReady")}</h4>
                <p className="mb-4 text-muted-foreground text-sm">{t("users.invite.linkReadyDescription")}</p>
                <div className="space-y-2">
                  <Label htmlFor="invite-url">{t("users.invite.copyLink")}</Label>
                  <div className="flex gap-2">
                    <Input id="invite-url" value={inviteUrl} readOnly className="font-mono text-sm" />
                    <Button type="button" variant="outline" size="icon" onClick={handleCopy} className="shrink-0">
                      {copied ? <IconCheck size={18} /> : <IconCopy size={18} />}
                    </Button>
                  </div>
                  <p className="text-muted-foreground text-xs">{t("users.invite.expiresIn")}</p>
                </div>
              </div>

              <div className="flex justify-end gap-2">
                <Button variant="outline" onClick={handleClose}>
                  {t("users.invite.close")}
                </Button>
              </div>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
