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

      // Convert to tree format and exclude folders being moved
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

  // Load folders when modal opens
  useEffect(() => {
    if (isOpen) {
      loadFolders();
      // Pre-select current folder if provided, otherwise select root
      if (currentFolderId) {
        setSelectedItems([currentFolderId]);
      } else {
        setSelectedItems(["root"]);
      }
    }
  }, [isOpen, loadFolders, currentFolderId]);

  // Get the first item being moved to auto-expand to its location
  const firstItemToMove = useMemo(() => {
    if (!itemsToMove) return null;
    if (itemsToMove.files.length > 0) return itemsToMove.files[0];
    if (itemsToMove.folders.length > 0) return itemsToMove.folders[0];
    return null;
  }, [itemsToMove]);

  // Add root folder option to the tree structure
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

      // Only allow selecting one folder for move destination
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
    // Single selection is now handled by FileTree
    setSelectedItems(newSelection);
  };

  const selectedFolder =
    selectedItems.length > 0 && selectedItems[0] !== "root"
      ? folders.find((f) => f.id === selectedItems[0])?.name
      : null;

  const itemCount = (itemsToMove?.files.length || 0) + (itemsToMove?.folders.length || 0);

  return (
    <Dialog open={isOpen} onOpenChange={handleClose}>
      <DialogContent className="max-w-2xl max-h-[80vh] w-full">
        <DialogHeader>
          <DialogTitle>{title || t("moveItems.title", { count: itemCount })}</DialogTitle>
          <DialogDescription>{description || t("moveItems.description", { count: itemCount })}</DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-4 flex-1 min-h-0 w-full overflow-hidden">
          {/* Items being moved */}
          <div className="text-sm">
            <div className="font-medium mb-2">{t("moveItems.itemsToMove")}</div>
            <div className="max-h-20 overflow-y-auto text-muted-foreground">
              {itemsToMove?.folders.map((folder) => (
                <div key={folder.id} className="flex items-center gap-2 truncate">
                  <IconFolder className="h-4 w-4 text-primary flex-shrink-0" />
                  <span className="truncate">{folder.name}</span>
                </div>
              ))}
              {itemsToMove?.files.map((file) => {
                const { icon: FileIcon, color } = getFileIcon(file.name);
                return (
                  <div key={file.id} className="flex items-center gap-2 truncate">
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
            <div className="text-sm text-muted-foreground mb-2">
              {selectedItems.length > 0 && selectedItems[0] !== "root" ? (
                <span>
                  {t("moveItems.movingTo")} {selectedFolder}
                </span>
              ) : (
                <span>
                  {t("moveItems.movingTo")} {t("folderActions.rootFolder")}
                </span>
              )}
            </div>
          </div>

          {/* Folder Tree */}
          <div className="flex-1 min-h-0 w-full overflow-hidden">
            {isLoading ? (
              <div className="flex items-center justify-center py-8">
                <div className="text-sm text-muted-foreground">{t("common.loadingSimple")}</div>
              </div>
            ) : (
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
                maxHeight="300px"
                singleSelection={true}
                useCheckboxAsRadio={true}
                searchQuery={searchQuery}
                autoExpandToItem={firstItemToMove?.id || null}
              />
            )}
          </div>
        </div>

        <DialogFooter>
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
