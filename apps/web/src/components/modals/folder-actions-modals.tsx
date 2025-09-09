import { IconEdit, IconFolderPlus, IconTrash } from "@tabler/icons-react";
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
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

interface FolderToEdit {
  id: string;
  name: string;
  description?: string | null;
}

interface FolderToDelete {
  id: string;
  name: string;
}

interface FolderActionsModalsProps {
  folderToCreate: boolean;
  onCreateFolder: (name: string, description?: string) => Promise<void>;
  onCloseCreate: () => void;

  folderToEdit: FolderToEdit | null;
  onEditFolder: (folderId: string, newName: string, description?: string) => Promise<void>;
  onCloseEdit: () => void;

  folderToDelete: FolderToDelete | null;
  onDeleteFolder: (folderId: string) => Promise<void>;
  onCloseDelete: () => void;
}

export function FolderActionsModals({
  folderToCreate,
  onCreateFolder,
  onCloseCreate,
  folderToEdit,
  onEditFolder,
  onCloseEdit,
  folderToDelete,
  onDeleteFolder,
  onCloseDelete,
}: FolderActionsModalsProps) {
  const t = useTranslations();

  return (
    <>
      <Dialog open={folderToCreate} onOpenChange={() => onCloseCreate()}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <IconFolderPlus size={20} />
              {t("folderActions.createFolder")}
            </DialogTitle>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <Input
              placeholder={t("folderActions.folderNamePlaceholder")}
              onKeyUp={(e) => {
                if (e.key === "Enter") {
                  const nameInput = e.currentTarget;
                  const descInput = document.querySelector(
                    `textarea[placeholder="${t("folderActions.folderDescriptionPlaceholder")}"]`
                  ) as HTMLTextAreaElement;

                  if (nameInput.value.trim()) {
                    onCreateFolder(nameInput.value.trim(), descInput?.value.trim() || undefined);
                  }
                }
              }}
            />
            <Textarea placeholder={t("folderActions.folderDescriptionPlaceholder")} />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={onCloseCreate}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                const nameInput = document.querySelector(
                  `input[placeholder="${t("folderActions.folderNamePlaceholder")}"]`
                ) as HTMLInputElement;
                const descInput = document.querySelector(
                  `textarea[placeholder="${t("folderActions.folderDescriptionPlaceholder")}"]`
                ) as HTMLTextAreaElement;

                if (nameInput && nameInput.value.trim()) {
                  onCreateFolder(nameInput.value.trim(), descInput?.value.trim() || undefined);
                }
              }}
            >
              {t("folderActions.createFolder")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={!!folderToEdit} onOpenChange={() => onCloseEdit()}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <IconEdit size={20} />
              {t("folderActions.editFolder")}
            </DialogTitle>
          </DialogHeader>
          {folderToEdit && (
            <div className="flex flex-col gap-4">
              <Input
                defaultValue={folderToEdit.name}
                placeholder={t("folderActions.folderNamePlaceholder")}
                onKeyUp={(e) => {
                  if (e.key === "Enter" && folderToEdit) {
                    const nameInput = e.currentTarget;
                    const descInput = document.querySelector(
                      `textarea[placeholder="${t("folderActions.folderDescriptionPlaceholder")}"]`
                    ) as HTMLTextAreaElement;

                    if (nameInput.value.trim()) {
                      onEditFolder(folderToEdit.id, nameInput.value.trim(), descInput?.value.trim() || undefined);
                    }
                  }
                }}
              />
              <Textarea
                defaultValue={folderToEdit.description || ""}
                placeholder={t("folderActions.folderDescriptionPlaceholder")}
              />
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={onCloseEdit}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                const nameInput = document.querySelector(
                  `input[placeholder="${t("folderActions.folderNamePlaceholder")}"]`
                ) as HTMLInputElement;
                const descInput = document.querySelector(
                  `textarea[placeholder="${t("folderActions.folderDescriptionPlaceholder")}"]`
                ) as HTMLTextAreaElement;

                if (folderToEdit && nameInput && nameInput.value.trim()) {
                  onEditFolder(folderToEdit.id, nameInput.value.trim(), descInput?.value.trim() || undefined);
                }
              }}
            >
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={!!folderToDelete} onOpenChange={() => onCloseDelete()}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <IconTrash size={20} />
              {t("folderActions.deleteFolder")}
            </DialogTitle>
          </DialogHeader>
          <DialogDescription>
            <p className="text-base font-semibold mb-2 text-foreground">{t("folderActions.deleteConfirmation")}</p>
            <p>
              {(folderToDelete?.name &&
                (folderToDelete.name.length > 50
                  ? folderToDelete.name.substring(0, 50) + "..."
                  : folderToDelete.name)) ||
                ""}
            </p>
            <p className="text-sm mt-2 text-amber-500">{t("folderActions.deleteWarning")}</p>
          </DialogDescription>
          <DialogFooter>
            <Button variant="outline" onClick={onCloseDelete}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" onClick={() => folderToDelete && onDeleteFolder(folderToDelete.id)}>
              {t("common.delete")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
