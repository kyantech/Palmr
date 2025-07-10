"use client";

import { useEffect, useState } from "react";
import { IconEye, IconEyeOff, IconLock, IconLockOpen } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Loader } from "@/components/ui/loader";
import { Switch } from "@/components/ui/switch";
import { updateSharePassword } from "@/http/endpoints";

interface ShareSecurityModalProps {
  shareId: string | null;
  share: any;
  onClose: () => void;
  onSuccess?: () => void;
}

export function ShareSecurityModal({ shareId, share, onClose, onSuccess }: ShareSecurityModalProps) {
  const t = useTranslations();
  const [isLoading, setIsLoading] = useState(false);
  const [hasPassword, setHasPassword] = useState(false);
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);

  useEffect(() => {
    if (share?.security) {
      const hasCurrentPassword = share.security.hasPassword || false;
      setHasPassword(hasCurrentPassword);
      setPassword("");
    }
  }, [share]);

  const handleSave = async () => {
    if (!shareId) return;

    if (hasPassword) {
      if (!password.trim()) {
        toast.error(t("shareSecurity.validation.passwordRequired"));
        return;
      }
      if (password.length < 2) {
        toast.error(t("shareSecurity.validation.passwordTooShort"));
        return;
      }
    }

    setIsLoading(true);
    try {
      await updateSharePassword(shareId, {
        password: hasPassword ? password : null,
      });

      const successMessage = hasPassword
        ? share?.security?.hasPassword
          ? t("shareSecurity.success.passwordUpdated")
          : t("shareSecurity.success.passwordSet")
        : t("shareSecurity.success.passwordRemoved");

      toast.success(successMessage);

      if (onSuccess) {
        onSuccess();
      }
      onClose();
    } catch (error) {
      console.error("Failed to update share security:", error);
      toast.error(t("shareSecurity.error.updateFailed"));
    } finally {
      setIsLoading(false);
    }
  };

  const handlePasswordToggle = (checked: boolean) => {
    setHasPassword(checked);
    if (!checked) {
      setPassword("");
      setShowPassword(false);
    }
  };

  const togglePasswordVisibility = () => {
    setShowPassword(!showPassword);
  };

  const getButtonText = () => {
    if (isLoading) {
      return (
        <div className="flex items-center gap-2">
          <Loader size="sm" />
          {share?.security?.hasPassword ? t("common.updating") : t("common.saving")}
        </div>
      );
    }
    return share?.security?.hasPassword ? t("common.update") : t("common.save");
  };

  return (
    <Dialog open={!!shareId} onOpenChange={() => onClose()}>
      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle>{t("shareSecurity.title")}</DialogTitle>
          <DialogDescription>{t("shareSecurity.subtitle")}</DialogDescription>
        </DialogHeader>

        <div className="py-4 space-y-6">
          <div className="space-y-3">
            <h3 className="text-sm font-medium text-foreground">{t("shareSecurity.currentStatus")}</h3>
            <div className="flex gap-2">
              {share?.security?.hasPassword ? (
                <Badge
                  variant="secondary"
                  className="bg-yellow-500/20 text-yellow-800 border-yellow-300 dark:bg-yellow-500/10 dark:text-yellow-400 dark:border-yellow-500/20"
                >
                  <IconLock className="h-3 w-3 mr-1" />
                  {t("shareDetails.passwordProtected")}
                </Badge>
              ) : (
                <Badge
                  variant="secondary"
                  className="bg-green-500/20 text-green-800 border-green-300 dark:bg-green-500/10 dark:text-green-400 dark:border-green-500/20"
                >
                  <IconLockOpen className="h-3 w-3 mr-1" />
                  {t("shareDetails.publicAccess")}
                </Badge>
              )}
            </div>
          </div>

          <div className="space-y-4">
            <div className="flex items-center space-x-2">
              <Switch id="password-protection" checked={hasPassword} onCheckedChange={handlePasswordToggle} />
              <Label htmlFor="password-protection" className="flex items-center gap-2">
                <IconLock size={16} />
                {t("shareSecurity.passwordProtection")}
              </Label>
            </div>

            {hasPassword && (
              <div className="space-y-4 pl-6 border-l-2 border-muted">
                {share?.security?.hasPassword && (
                  <div className="bg-muted/50 border border-border rounded-lg p-3">
                    <p className="text-sm text-muted-foreground">{t("shareSecurity.existingPasswordMessage")}</p>
                  </div>
                )}

                <div className="space-y-2">
                  <Label htmlFor="password">
                    {share?.security?.hasPassword ? t("shareSecurity.newPassword") : t("shareSecurity.password")}
                  </Label>
                  <div className="relative">
                    <Input
                      id="password"
                      type={showPassword ? "text" : "password"}
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      placeholder={t("shareSecurity.passwordPlaceholder")}
                      className="pr-10"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="absolute right-0 top-0 h-full w-10 hover:bg-transparent"
                      onClick={togglePasswordVisibility}
                    >
                      {showPassword ? (
                        <IconEyeOff className="h-4 w-4 text-muted-foreground hover:text-foreground" />
                      ) : (
                        <IconEye className="h-4 w-4 text-muted-foreground hover:text-foreground" />
                      )}
                    </Button>
                  </div>
                </div>

                <div className="text-xs text-muted-foreground space-y-1">
                  <p>{t("shareSecurity.passwordRequirements.title")}</p>
                  <ul className="list-disc list-inside space-y-1 ml-2">
                    <li>{t("shareSecurity.passwordRequirements.minLength")}</li>
                  </ul>
                </div>
              </div>
            )}
          </div>

          <div className="bg-muted/30 p-3 rounded-lg">
            <div className="text-sm space-y-1">
              <p className="font-medium text-muted-foreground">{t("shareSecurity.info.title")}</p>
              <p className="text-muted-foreground">
                {hasPassword ? t("shareSecurity.info.withPassword") : t("shareSecurity.info.withoutPassword")}
              </p>
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={isLoading}>
            {t("common.cancel")}
          </Button>
          <Button onClick={handleSave} disabled={isLoading}>
            {getButtonText()}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
