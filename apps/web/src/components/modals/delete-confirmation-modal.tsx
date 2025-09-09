"use client";

import { IconFolder, IconTrash, IconX } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { getFileIcon } from "@/utils/file-icons";
import { truncateFileName } from "@/utils/file-utils";

interface DeleteConfirmationModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: () => void;
  title: string;
  description: string;
  files?: string[];
  folders?: string[];
  itemType?: "files" | "shares" | "mixed";
}

export function DeleteConfirmationModal({
  isOpen,
  onClose,
  onConfirm,
  title,
  description,
  files = [],
  folders = [],
  itemType,
}: DeleteConfirmationModalProps) {
  const t = useTranslations();

  const handleConfirm = () => {
    onConfirm();
    onClose();
  };

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-destructive">
            <IconTrash size={20} />
            {title}
          </DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <p className="text-sm text-muted-foreground">{description}</p>

          {(files.length > 0 || folders.length > 0) && (
            <div className="space-y-2">
              <p className="text-sm font-medium">
                {itemType === "shares"
                  ? t("deleteConfirmation.sharesToDelete")
                  : folders.length > 0 && files.length > 0
                    ? t("deleteConfirmation.itemsToDelete")
                    : folders.length > 0
                      ? t("deleteConfirmation.foldersToDelete")
                      : t("deleteConfirmation.filesToDelete")}
                :
              </p>
              <ScrollArea className="h-32 w-full rounded-md border p-2">
                <div className="space-y-1">
                  {folders.map((folderName, index) => (
                    <div
                      key={`folder-${index}`}
                      className="flex items-center gap-2 p-2 bg-muted/20 rounded text-sm min-w-0"
                    >
                      <IconFolder className="h-4 w-4 text-primary flex-shrink-0" />
                      <span className="flex-1 break-all" title={folderName}>
                        {truncateFileName(folderName)}
                      </span>
                    </div>
                  ))}
                  {files.map((fileName, index) => {
                    const { icon: FileIcon, color } = getFileIcon(fileName);
                    const displayName = truncateFileName(fileName);
                    return (
                      <div
                        key={`file-${index}`}
                        className="flex items-center gap-2 p-2 bg-muted/20 rounded text-sm min-w-0"
                      >
                        <FileIcon className={`h-4 w-4 ${color} flex-shrink-0`} />
                        <span className="flex-1 break-all" title={fileName}>
                          {displayName}
                        </span>
                      </div>
                    );
                  })}
                </div>
              </ScrollArea>
            </div>
          )}
        </div>

        <DialogFooter className="flex gap-2">
          <Button variant="outline" onClick={onClose}>
            <IconX className="h-4 w-4" />
            {t("common.cancel")}
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            <IconTrash className="h-4 w-4" />
            {t("common.delete")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
