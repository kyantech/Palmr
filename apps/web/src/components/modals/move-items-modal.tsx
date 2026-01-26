"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { IconFolder } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { FileTree, TreeFolder } from "@/components/tables/files-tree";
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
import { getFileIcon } from "@/utils/file-icons";

interface MoveItemsModalProps {
  isOpen: boolean;
  onClose: () => void;
  onMove: (targetFolderId: string | null) => Promise<void>;
  itemsToMove: { files: any[]; folders: any[] } | null;
  title?: string;
  description?: string;
  getAllFolders: () => Promise<any[]>;
  currentFolderId?: string | null;
}

export function MoveItemsModal({
  isOpen,
  onClose,
  onMove,
  itemsToMove,
  title,
  description,
  getAllFolders,
  currentFolderId = null,
}: MoveItemsModalProps) {
  const t = useTranslations();
  const [selectedItems, setSelectedItems] = useState<string[]>([]);
  const [folders, setFolders] = useState<TreeFolder[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isMoving, setIsMoving] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");

  const loadFolders = useCallback(async () => {
    try {
      setIsLoading(true);
      const data = await getAllFolders();

      const excludedIds = new Set(itemsToMove?.folders.map((f) => f.id) || []);

      const treeFolders: TreeFolder[] = data
        .filter((folder) => !excludedIds.has(folder.id))
        .map((folder) => ({
          id: folder.id,
          name: folder.name,
          type: "folder" as const,
          parentId: folder.parentId || null,
          totalSize: folder.totalSize,
        }));

      setFolders(treeFolders);
    } catch (error) {
      console.error("Error loading folders:", error);
    } finally {
      setIsLoading(false);
    }
  }, [getAllFolders, itemsToMove]);

  useEffect(() => {
    if (isOpen) {
      loadFolders();

      if (currentFolderId) {
        setSelectedItems([currentFolderId]);
      } else {
        setSelectedItems(["root"]);
      }
    }
  }, [isOpen, loadFolders, currentFolderId]);

  const firstItemToMove = useMemo(() => {
    if (!itemsToMove) return null;
    if (itemsToMove.files.length > 0) return itemsToMove.files[0];
    if (itemsToMove.folders.length > 0) return itemsToMove.folders[0];
    return null;
  }, [itemsToMove]);

  const foldersWithRoot = useMemo(() => {
    const rootFolder: TreeFolder = {
      id: "root",
      name: t("folderActions.rootFolder"),
      type: "folder" as const,
      parentId: null,
    };
    return [rootFolder, ...folders];
  }, [folders, t]);

  const handleMove = async () => {
    try {
      setIsMoving(true);

      const targetFolderId = selectedItems.length > 0 && selectedItems[0] !== "root" ? selectedItems[0] : null;

      await onMove(targetFolderId);
      onClose();
    } catch (error) {
      console.error("Error moving items:", error);
    } finally {
      setIsMoving(false);
    }
  };

  const handleClose = () => {
    if (!isMoving) {
      setSelectedItems([]);
      setSearchQuery("");
      onClose();
    }
  };

  const handleSelectionChange = (newSelection: string[]) => {
    setSelectedItems(newSelection);
  };

  const selectedFolder =
    selectedItems.length > 0 && selectedItems[0] !== "root"
      ? folders.find((f) => f.id === selectedItems[0])?.name
      : null;

  const itemCount = (itemsToMove?.files.length || 0) + (itemsToMove?.folders.length || 0);

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="max-w-2xl max-h-[85vh] w-full flex flex-col">
        <DialogHeader>
          <DialogTitle>{title || t("moveItems.title", { count: itemCount })}</DialogTitle>
          <DialogDescription>{description || t("moveItems.description", { count: itemCount })}</DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-4 flex-1 min-h-0 overflow-y-auto">
          {/* Items being moved */}
          <div className="text-sm">
            <div className="font-medium mb-2">{t("moveItems.itemsToMove")}</div>
            <div className="max-h-20 overflow-y-auto text-muted-foreground border rounded-md p-2">
              {itemsToMove?.folders.map((folder) => (
                <div key={folder.id} className="flex items-center gap-2 truncate py-1">
                  <IconFolder className="h-4 w-4 text-primary flex-shrink-0" />
                  <span className="truncate">{folder.name}</span>
                </div>
              ))}
              {itemsToMove?.files.map((file) => {
                const { icon: FileIcon, color } = getFileIcon(file.name);
                return (
                  <div key={file.id} className="flex items-center gap-2 truncate py-1">
                    <FileIcon className={`h-4 w-4 ${color} flex-shrink-0`} />
                    <span className="truncate">{file.name}</span>
                  </div>
                );
              })}
            </div>
          </div>

          {/* Search */}
          <div className="space-y-2">
            <Label htmlFor="search">{t("common.search")}</Label>
            <Input
              id="search"
              type="search"
              placeholder={t("searchBar.placeholderFolders")}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              disabled={isLoading}
            />
          </div>

          {/* Destination Selection */}
          <div className="space-y-2">
            <Label>{t("folderActions.selectDestination")}</Label>
            <div className="text-sm text-muted-foreground">
              {selectedItems.length > 0 && selectedItems[0] !== "root" ? (
                <span>
                  {t("moveItems.movingTo")} <strong>{selectedFolder}</strong>
                </span>
              ) : (
                <span>
                  {t("moveItems.movingTo")} <strong>{t("folderActions.rootFolder")}</strong>
                </span>
              )}
            </div>
          </div>

          {/* Folder Tree */}
          <div className="flex-1 min-h-[200px] max-h-[350px] border rounded-md overflow-hidden">
            {isLoading ? (
              <div className="flex items-center justify-center py-8">
                <div className="text-sm text-muted-foreground">{t("common.loadingSimple")}</div>
              </div>
            ) : (
              <div className="h-full overflow-y-auto">
                <FileTree
                  files={[]}
                  folders={foldersWithRoot.map((folder: TreeFolder) => ({
                    id: folder.id,
                    name: folder.name,
                    type: "folder" as const,
                    parentId: folder.parentId || null,
                    description: "",
                    userId: "",
                    createdAt: "",
                    updatedAt: "",
                    totalSize: folder.totalSize,
                  }))}
                  selectedItems={selectedItems}
                  onSelectionChange={handleSelectionChange}
                  showFiles={false}
                  showFolders={true}
                  singleSelection={true}
                  useCheckboxAsRadio={true}
                  searchQuery={searchQuery}
                  autoExpandToItem={firstItemToMove?.id || null}
                />
              </div>
            )}
          </div>
        </div>

        <DialogFooter className="mt-4">
          <Button variant="outline" onClick={handleClose} disabled={isMoving}>
            {t("common.cancel")}
          </Button>
          <Button onClick={handleMove} disabled={isLoading || isMoving}>
            {isMoving ? t("common.move") + "..." : t("common.move")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
