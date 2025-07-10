import { IconAlertTriangle, IconTrash } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ReverseShare } from "../hooks/use-reverse-shares";

interface DeleteReverseShareModalProps {
  reverseShare: ReverseShare | null;
  isDeleting: boolean;
  onClose: () => void;
  onConfirm: (reverseShare: ReverseShare) => Promise<void>;
}

export function DeleteReverseShareModal({
  reverseShare,
  isDeleting,
  onClose,
  onConfirm,
}: DeleteReverseShareModalProps) {
  const t = useTranslations();

  if (!reverseShare) return null;

  const handleConfirm = async () => {
    await onConfirm(reverseShare);
  };

  return (
    <Dialog open={!!reverseShare} onOpenChange={() => !isDeleting && onClose()}>
      <DialogContent className="sm:max-w-[450px]">
        <DialogHeader>
          <div className="flex items-center gap-3">
            <div className="flex h-10 w-10 items-center justify-center rounded-full bg-destructive/10">
              <IconAlertTriangle className="h-5 w-5 text-destructive" />
            </div>
            <div>
              <DialogTitle className="text-left">{t("reverseShares.delete.title")}</DialogTitle>
            </div>
          </div>
          <DialogDescription className="text-left pt-2">{t("reverseShares.delete.description")}</DialogDescription>
        </DialogHeader>

        {/* Informações do reverse-share a ser excluído */}
        <div className="rounded-lg border p-4 bg-muted/30">
          <div className="space-y-2">
            <h4 className="font-medium">{reverseShare.name || t("reverseShares.card.untitled")}</h4>
            {reverseShare.description && <p className="text-sm text-muted-foreground">{reverseShare.description}</p>}
            <div className="flex items-center gap-4 text-xs text-muted-foreground">
              <span>
                {reverseShare.files?.length || 0} {t("reverseShares.labels.filesReceived")}
              </span>
              {reverseShare.alias?.alias && <span>Link: /r/{reverseShare.alias.alias}</span>}
            </div>
          </div>
        </div>

        <DialogFooter className="gap-2">
          <Button variant="outline" onClick={onClose} disabled={isDeleting}>
            {t("reverseShares.delete.cancelButton")}
          </Button>
          <Button variant="destructive" onClick={handleConfirm} disabled={isDeleting}>
            {isDeleting ? (
              <>
                <IconTrash className="h-4 w-4 animate-spin" />
                {t("reverseShares.delete.deleting")}
              </>
            ) : (
              <>
                <IconTrash className="h-4 w-4" />
                {t("reverseShares.delete.confirmButton")}
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
