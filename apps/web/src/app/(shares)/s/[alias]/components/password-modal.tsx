import { IconLock } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { PasswordModalProps } from "../types";

export function PasswordModal({ isOpen, password, isError, onPasswordChange, onSubmit }: PasswordModalProps) {
  const t = useTranslations();

  return (
    <Dialog open={isOpen} onOpenChange={() => {}} modal>
      <DialogContent>
        <DialogHeader className="flex flex-col gap-1">
          <DialogTitle>{t("share.password.title")}</DialogTitle>
          <div className="flex items-center gap-2 text-warning text-sm">
            <IconLock size={16} />
            <p>{t("share.password.protected")}</p>
          </div>
          {isError && (
            <div className="flex items-center gap-2 text-destructive text-sm mt-2">
              <p>{t("share.password.incorrect")}</p>
            </div>
          )}
        </DialogHeader>
        <div className="py-4">
          <Input
            type="password"
            value={password}
            placeholder={t("share.password.placeholder")}
            onChange={(e) => onPasswordChange(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && onSubmit()}
          />
        </div>
        <DialogFooter>
          <Button onClick={onSubmit}>{t("share.password.submit")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
