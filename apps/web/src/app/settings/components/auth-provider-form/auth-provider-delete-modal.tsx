import { IconTrash } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

interface AuthProviderDeleteModalProps {
  isOpen: boolean;
  onClose: () => void;
  provider: { id: string; name: string; displayName: string } | null;
  onConfirm: () => Promise<void>;
  isDeleting: boolean;
}

export function AuthProviderDeleteModal({
  isOpen,
  onClose,
  provider,
  onConfirm,
  isDeleting,
}: AuthProviderDeleteModalProps) {
  const t = useTranslations();

  const handleConfirm = async () => {
    await onConfirm();
  };

  return (
    <Dialog open={isOpen} onOpenChange={() => !isDeleting && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-destructive">
            <IconTrash size={20} />
            {t("authProviders.deleteModal.title")}
          </DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <p className="text-sm text-muted-foreground">
            {t("authProviders.deleteModal.confirmMessage", { displayName: provider?.displayName || "" })}
          </p>

          {provider && (
            <div className="rounded-lg border p-4 bg-muted/30">
              <div className="space-y-2">
                <h4 className="font-medium">{provider.displayName}</h4>
                <p className="text-sm text-muted-foreground">
                  {t("authProviders.deleteModal.providerId", { name: provider.name })}
                </p>
              </div>
            </div>
          )}
        </div>

        <DialogFooter className="flex gap-2">
          <Button variant="outline" onClick={onClose} disabled={isDeleting}>
            {t("authProviders.deleteModal.cancel")}
          </Button>
          <Button variant="destructive" onClick={handleConfirm} disabled={isDeleting}>
            {isDeleting ? (
              <>
                <IconTrash className="h-4 w-4 animate-spin" />
                {t("authProviders.deleteModal.deleting")}
              </>
            ) : (
              <>
                <IconTrash className="h-4 w-4" />
                {t("authProviders.deleteModal.delete")}
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
