"use client";

import React, { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { RecipientSelector } from "@/components/general/recipient-selector";
import { FileTree, TreeFile, TreeFolder } from "@/components/tables/files-tree";
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
import { Switch } from "@/components/ui/switch";
import { addFiles, addFolders, removeFiles, removeFolders, updateSharePassword } from "@/http/endpoints";
import { listFolders } from "@/http/endpoints/folders";

export interface ShareActionsModalsProps {
  shareToDelete: any;
  shareToEdit: any;
  shareToManageFiles: any;
  shareToManageRecipients: any;
  onCloseDelete: () => void;
  onCloseEdit: () => void;
  onCloseManageFiles: () => void;
  onCloseManageRecipients: () => void;
  onDelete: (shareId: string) => Promise<void>;
  onEdit: (shareId: string, data: any) => Promise<void>;
  onManageFiles: (shareId: string, files: string[], folders: string[]) => Promise<void>;
  onManageRecipients: (shareId: string, recipients: string[]) => Promise<void>;
  onEditFile?: (fileId: string, newName: string, description?: string) => Promise<void>;
  onEditFolder?: (folderId: string, newName: string, description?: string) => Promise<void>;
  onSuccess: () => void;
}

export function ShareActionsModals({
  shareToDelete,
  shareToEdit,
  shareToManageFiles,
  shareToManageRecipients,
  onCloseDelete,
  onCloseEdit,
  onCloseManageFiles,
  onCloseManageRecipients,
  onDelete,
  onEdit,
  onSuccess,
}: ShareActionsModalsProps) {
  const t = useTranslations();
  const [isLoading, setIsLoading] = useState(false);
  const [allFiles, setAllFiles] = useState<any[]>([]);
  const [allFolders, setAllFolders] = useState<any[]>([]);

  const [manageFilesSelectedItems, setManageFilesSelectedItems] = useState<string[]>([]);
  const [manageFilesTreeFiles, setManageFilesTreeFiles] = useState<TreeFile[]>([]);
  const [manageFilesTreeFolders, setManageFilesTreeFolders] = useState<TreeFolder[]>([]);
  const [isManageFilesLoading, setIsManageFilesLoading] = useState(false);
  const [isManageFilesSaving, setIsManageFilesSaving] = useState(false);
  const [manageFilesSearchQuery, setManageFilesSearchQuery] = useState("");

  React.useEffect(() => {
    const loadAllData = async () => {
      try {
        const [allFoldersResponse, allFilesResponse] = await Promise.all([
          listFolders(),
          fetch("/api/files?recursive=true").then((res) => res.json()),
        ]);
        setAllFolders(allFoldersResponse.data.folders || []);
        setAllFiles(allFilesResponse.files || []);
      } catch (error) {
        console.error("Error loading all files and folders:", error);
        setAllFolders([]);
        setAllFiles([]);
      }
    };

    loadAllData();
  }, []);

  const [editForm, setEditForm] = useState({
    name: "",
    description: "",
    expiresAt: "",
    isPasswordProtected: false,
    password: "",
    maxViews: "",
  });

  useEffect(() => {
    if (shareToEdit) {
      setEditForm({
        name: shareToEdit.name || "",
        description: shareToEdit.description || "",
        expiresAt: shareToEdit.expiration ? new Date(shareToEdit.expiration).toISOString().slice(0, 16) : "",
        isPasswordProtected: Boolean(shareToEdit.security?.hasPassword),
        password: "",
        maxViews: shareToEdit.security?.maxViews?.toString() || "",
      });
    }
  }, [shareToEdit]);

  const loadManageFilesData = useCallback(async () => {
    try {
      setIsManageFilesLoading(true);

      const treeFiles: TreeFile[] = allFiles.map((file) => ({
        id: file.id,
        name: file.name,
        type: "file" as const,
        size: file.size,
        parentId: file.folderId || null,
      }));

      const treeFolders: TreeFolder[] = allFolders.map((folder) => ({
        id: folder.id,
        name: folder.name,
        type: "folder" as const,
        parentId: folder.parentId || null,
        totalSize: folder.totalSize,
      }));

      setManageFilesTreeFiles(treeFiles);
      setManageFilesTreeFolders(treeFolders);
    } catch (error) {
      console.error("Error loading files and folders:", error);
    } finally {
      setIsManageFilesLoading(false);
    }
  }, [allFiles, allFolders]);

  useEffect(() => {
    if (shareToManageFiles) {
      loadManageFilesData();

      const initialSelectedFiles = shareToManageFiles?.files?.map((f: any) => f.id) || [];
      const initialSelectedFolders = shareToManageFiles?.folders?.map((f: any) => f.id) || [];
      setManageFilesSelectedItems([...initialSelectedFiles, ...initialSelectedFolders]);
    }
  }, [shareToManageFiles, loadManageFilesData]);

  const handleDelete = async () => {
    if (!shareToDelete) return;
    setIsLoading(true);
    await onDelete(shareToDelete.id);
    setIsLoading(false);
  };

  const handleEdit = async () => {
    if (!shareToEdit) return;
    setIsLoading(true);

    try {
      const updateData = {
        name: editForm.name,
        description: editForm.description,
        expiration: editForm.expiresAt ? new Date(editForm.expiresAt).toISOString() : undefined,
        maxViews: editForm.maxViews ? parseInt(editForm.maxViews) : null,
      };

      await onEdit(shareToEdit.id, updateData);

      if (!editForm.isPasswordProtected && shareToEdit.security.hasPassword) {
        await updateSharePassword(shareToEdit.id, { password: "" });
      } else if (editForm.isPasswordProtected && editForm.password) {
        await updateSharePassword(shareToEdit.id, { password: editForm.password });
      }

      onSuccess();
      onCloseEdit();
      toast.success(t("shareActions.editSuccess"));
    } catch {
      toast.error(t("shareActions.editError"));
    } finally {
      setIsLoading(false);
    }
  };

  const handleManageFilesSave = async () => {
    if (!shareToManageFiles?.id) return;

    try {
      setIsManageFilesSaving(true);

      const selectedFiles = manageFilesSelectedItems.filter((id) =>
        manageFilesTreeFiles.some((file) => file.id === id)
      );
      const selectedFolders = manageFilesSelectedItems.filter((id) =>
        manageFilesTreeFolders.some((folder) => folder.id === id)
      );

      const currentFileIds = shareToManageFiles.files?.map((f: any) => f.id) || [];
      const currentFolderIds = shareToManageFiles.folders?.map((f: any) => f.id) || [];

      const filesToAdd = selectedFiles.filter((id: string) => !currentFileIds.includes(id));
      const filesToRemove = currentFileIds.filter((id: string) => !selectedFiles.includes(id));

      const foldersToAdd = selectedFolders.filter((id: string) => !currentFolderIds.includes(id));
      const foldersToRemove = currentFolderIds.filter((id: string) => !selectedFolders.includes(id));

      const promises = [];

      if (filesToAdd.length > 0) {
        promises.push(addFiles(shareToManageFiles.id, { files: filesToAdd }));
      }
      if (filesToRemove.length > 0) {
        promises.push(removeFiles(shareToManageFiles.id, { files: filesToRemove }));
      }
      if (foldersToAdd.length > 0) {
        promises.push(addFolders(shareToManageFiles.id, { folders: foldersToAdd }));
      }
      if (foldersToRemove.length > 0) {
        promises.push(removeFolders(shareToManageFiles.id, { folders: foldersToRemove }));
      }

      await Promise.all(promises);
      onSuccess();
      onCloseManageFiles();
      toast.success(t("shareActions.editSuccess"));
    } catch (error) {
      console.error("Error updating share files:", error);
      toast.error(t("shareActions.editError"));
      throw error;
    } finally {
      setIsManageFilesSaving(false);
    }
  };

  const handleManageFilesClose = () => {
    if (!isManageFilesSaving) {
      setManageFilesSelectedItems([]);
      setManageFilesSearchQuery("");
      onCloseManageFiles();
    }
  };

  return (
    <>
      <Dialog open={!!shareToDelete} onOpenChange={() => onCloseDelete()}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("shareActions.deleteTitle")}</DialogTitle>
            <DialogDescription>{t("shareActions.deleteConfirmation")}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={onCloseDelete}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" disabled={isLoading} onClick={handleDelete}>
              {t("common.delete")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={!!shareToEdit} onOpenChange={() => onCloseEdit()}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("shareActions.editTitle")}</DialogTitle>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            <div className="grid w-full items-center gap-1.5">
              <Label>{t("createShare.nameLabel")}</Label>
              <Input value={editForm.name} onChange={(e) => setEditForm({ ...editForm, name: e.target.value })} />
            </div>
            <div className="grid w-full items-center gap-1.5">
              <Label>{t("createShare.descriptionLabel")}</Label>
              <Input
                value={editForm.description}
                onChange={(e) => setEditForm({ ...editForm, description: e.target.value })}
                placeholder={t("createShare.descriptionPlaceholder")}
              />
            </div>
            <div className="grid w-full items-center gap-1.5">
              <Label>{t("createShare.expirationLabel")}</Label>
              <Input
                placeholder={t("createShare.expirationPlaceholder")}
                type="datetime-local"
                value={editForm.expiresAt}
                onChange={(e) => setEditForm({ ...editForm, expiresAt: e.target.value })}
              />
            </div>
            <div className="grid w-full items-center gap-1.5">
              <Label>{t("createShare.maxViewsLabel")}</Label>
              <Input
                min="1"
                placeholder={t("createShare.maxViewsPlaceholder")}
                type="number"
                value={editForm.maxViews}
                onChange={(e) => setEditForm({ ...editForm, maxViews: e.target.value })}
              />
            </div>
            <div className="flex items-center space-x-2">
              <Switch
                checked={editForm.isPasswordProtected}
                onCheckedChange={(checked) =>
                  setEditForm({
                    ...editForm,
                    isPasswordProtected: checked,
                    password: "",
                  })
                }
              />
              <Label>{t("createShare.passwordProtection")}</Label>
            </div>
            {editForm.isPasswordProtected && (
              <div className="grid w-full items-center gap-1.5">
                <Label>
                  {shareToEdit?.security?.hasPassword
                    ? t("shareActions.newPasswordLabel")
                    : t("createShare.passwordLabel")}
                </Label>
                <Input
                  placeholder={
                    shareToEdit?.security?.hasPassword
                      ? t("shareActions.newPasswordPlaceholder")
                      : t("createShare.passwordLabel")
                  }
                  type="password"
                  value={editForm.password}
                  onChange={(e) => setEditForm({ ...editForm, password: e.target.value })}
                />
              </div>
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={onCloseEdit}>
              {t("common.cancel")}
            </Button>
            <Button disabled={isLoading} onClick={handleEdit}>
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={!!shareToManageFiles} onOpenChange={handleManageFilesClose}>
        <DialogContent className="max-w-2xl max-h-[80vh] w-full">
          <DialogHeader>
            <DialogTitle>{t("shareActions.manageFilesTitle")}</DialogTitle>
            <DialogDescription>Select files and folders to include in this share</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-4 flex-1 min-h-0 w-full overflow-hidden">
            {/* Search */}
            <div className="space-y-2">
              <Label htmlFor="manage-files-search">{t("common.search")}</Label>
              <Input
                id="manage-files-search"
                type="search"
                placeholder={t("searchBar.placeholder")}
                value={manageFilesSearchQuery}
                onChange={(e) => setManageFilesSearchQuery(e.target.value)}
                disabled={isManageFilesLoading}
              />
            </div>

            {/* Selection Count */}
            <div className="text-sm text-muted-foreground">
              {manageFilesSelectedItems.length > 0 && <span>{manageFilesSelectedItems.length} items selected</span>}
            </div>

            {/* File Tree */}
            <div className="flex-1 min-h-0 w-full overflow-hidden">
              {isManageFilesLoading ? (
                <div className="flex items-center justify-center py-8">
                  <div className="text-sm text-muted-foreground">{t("common.loadingSimple")}</div>
                </div>
              ) : (
                <FileTree
                  files={manageFilesTreeFiles.map((file) => ({
                    id: file.id,
                    name: file.name,
                    description: "",
                    extension: "",
                    size: file.size?.toString() || "0",
                    objectName: "",
                    userId: "",
                    folderId: file.parentId,
                    createdAt: "",
                    updatedAt: "",
                  }))}
                  folders={manageFilesTreeFolders.map((folder) => ({
                    id: folder.id,
                    name: folder.name,
                    description: "",
                    parentId: folder.parentId,
                    userId: "",
                    createdAt: "",
                    updatedAt: "",
                    totalSize: folder.totalSize,
                  }))}
                  selectedItems={manageFilesSelectedItems}
                  onSelectionChange={setManageFilesSelectedItems}
                  showFiles={true}
                  showFolders={true}
                  maxHeight="400px"
                  searchQuery={manageFilesSearchQuery}
                />
              )}
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={handleManageFilesClose} disabled={isManageFilesSaving}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={handleManageFilesSave}
              disabled={isManageFilesLoading || isManageFilesSaving || manageFilesSelectedItems.length === 0}
            >
              {isManageFilesSaving ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={!!shareToManageRecipients} onOpenChange={() => onCloseManageRecipients()}>
        <DialogContent className="sm:max-w-[500px] md:max-w-[650px] max-h-[85vh] overflow-hidden">
          <DialogHeader className="space-y-3">
            <DialogTitle className="text-xl font-semibold">{t("shareActions.manageRecipientsTitle")}</DialogTitle>
            <DialogDescription className="text-muted-foreground">
              {t("recipientSelector.modalDescription")}
            </DialogDescription>
          </DialogHeader>
          <div className="overflow-y-auto max-h-[calc(85vh-140px)] py-2">
            <RecipientSelector
              selectedRecipients={shareToManageRecipients?.recipients || []}
              shareAlias={shareToManageRecipients?.alias?.alias}
              shareId={shareToManageRecipients?.id}
              onSuccess={onSuccess}
            />
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
