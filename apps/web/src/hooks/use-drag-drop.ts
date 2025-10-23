import { useCallback, useRef, useState } from "react";
import { toast } from "sonner";

import { moveFile } from "@/http/endpoints/files";
import { moveFolder } from "@/http/endpoints/folders";

interface DragItem {
  id: string;
  type: "file" | "folder";
  name: string;
}

interface DropTarget {
  id: string;
  type: "folder";
  name: string;
}

interface UseDragDropProps {
  onRefresh?: () => Promise<void>;
  onImmediateUpdate?: (itemId: string, itemType: "file" | "folder", newParentId: string | null) => void;
  selectedFiles?: Set<string>;
  selectedFolders?: Set<string>;
  files?: any[];
  folders?: any[];
}

export function useDragDrop({
  onRefresh,
  onImmediateUpdate,
  selectedFiles,
  selectedFolders,
  files,
  folders,
}: UseDragDropProps) {
  const [draggedItem, setDraggedItem] = useState<DragItem | null>(null);
  const [draggedItems, setDraggedItems] = useState<DragItem[]>([]);
  const [dragOverTarget, setDragOverTarget] = useState<DropTarget | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const dragCounter = useRef(0);

  const createDragGhost = useCallback((items: DragItem[]) => {
    const ghost = document.createElement("div");
    ghost.style.position = "absolute";
    ghost.style.top = "-9999px";
    ghost.style.left = "-9999px";
    ghost.style.display = "flex";
    ghost.style.flexDirection = "column";
    ghost.style.gap = "4px";
    ghost.style.padding = "12px";

    // Dark mode support
    const isDarkMode = document.documentElement.classList.contains("dark");
    ghost.style.backgroundColor = isDarkMode ? "rgba(10, 10, 10, 0.95)" : "rgba(255, 255, 255, 0.95)";
    ghost.style.border = "2px solid hsl(var(--primary))";
    ghost.style.borderRadius = "8px";
    ghost.style.boxShadow = isDarkMode
      ? "0 8px 16px rgba(0, 0, 0, 0.4), 0 0 0 1px rgba(255, 255, 255, 0.1)"
      : "0 8px 16px rgba(0, 0, 0, 0.15)";
    ghost.style.minWidth = "200px";
    ghost.style.maxWidth = "250px";
    ghost.style.zIndex = "9999";
    ghost.style.backdropFilter = "blur(12px)";

    const itemsToShow = items.slice(0, 3);
    const remaining = items.length - itemsToShow.length;

    itemsToShow.forEach((item, index) => {
      const itemDiv = document.createElement("div");
      itemDiv.style.display = "flex";
      itemDiv.style.alignItems = "center";
      itemDiv.style.gap = "8px";
      itemDiv.style.padding = "6px 8px";
      itemDiv.style.backgroundColor = isDarkMode ? "rgba(255, 255, 255, 0.05)" : "rgba(0, 0, 0, 0.03)";
      itemDiv.style.borderRadius = "4px";
      itemDiv.style.fontSize = "13px";
      itemDiv.style.fontWeight = "500";
      itemDiv.style.color = "hsl(var(--foreground))";
      itemDiv.style.transition = "all 0.2s ease";
      itemDiv.style.boxShadow = isDarkMode ? "0 1px 3px rgba(0, 0, 0, 0.3)" : "0 1px 3px rgba(0, 0, 0, 0.1)";

      const icon = document.createElement("span");
      icon.textContent = item.type === "folder" ? "📁" : "📄";
      icon.style.fontSize = "16px";

      const name = document.createElement("span");
      name.textContent = item.name;
      name.style.overflow = "hidden";
      name.style.textOverflow = "ellipsis";
      name.style.whiteSpace = "nowrap";

      itemDiv.appendChild(icon);
      itemDiv.appendChild(name);
      ghost.appendChild(itemDiv);
    });

    if (remaining > 0) {
      const moreDiv = document.createElement("div");
      moreDiv.style.padding = "4px 8px";
      moreDiv.style.fontSize = "12px";
      moreDiv.style.fontWeight = "600";
      moreDiv.style.color = "hsl(var(--primary))";
      moreDiv.style.textAlign = "center";
      moreDiv.style.backgroundColor = isDarkMode ? "rgba(255, 255, 255, 0.03)" : "rgba(0, 0, 0, 0.02)";
      moreDiv.style.borderRadius = "4px";
      moreDiv.style.marginTop = "2px";
      moreDiv.textContent = `+${remaining} more`;
      ghost.appendChild(moreDiv);
    }

    document.body.appendChild(ghost);
    return ghost;
  }, []);

  const handleDragStart = useCallback(
    (e: React.DragEvent, item: DragItem) => {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("application/x-move-item", "true");

      let itemsToDrag: DragItem[] = [item];

      // Check if item is in selection
      const isFileSelected = selectedFiles?.has(item.id) && item.type === "file";
      const isFolderSelected = selectedFolders?.has(item.id) && item.type === "folder";

      if (isFileSelected || isFolderSelected) {
        // Drag all selected items
        const selectedFileItems: DragItem[] =
          files
            ?.filter((f) => selectedFiles?.has(f.id))
            .map((f) => ({ id: f.id, type: "file" as const, name: f.name })) || [];
        const selectedFolderItems: DragItem[] =
          folders
            ?.filter((f) => selectedFolders?.has(f.id))
            .map((f) => ({ id: f.id, type: "folder" as const, name: f.name })) || [];

        itemsToDrag = [...selectedFolderItems, ...selectedFileItems];
      }

      e.dataTransfer.setData("text/plain", JSON.stringify(itemsToDrag));
      setDraggedItem(item);
      setDraggedItems(itemsToDrag);
      setIsDragging(true);

      // Create and set custom drag ghost (for single or multiple items)
      const ghost = createDragGhost(itemsToDrag);
      e.dataTransfer.setDragImage(ghost, 20, 20);

      // Clean up ghost after drag starts
      setTimeout(() => {
        document.body.removeChild(ghost);
      }, 0);
    },
    [selectedFiles, selectedFolders, files, folders, createDragGhost]
  );

  const handleDragEnd = useCallback(() => {
    setDraggedItem(null);
    setDraggedItems([]);
    setDragOverTarget(null);
    setIsDragging(false);
    dragCounter.current = 0;
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent, target: DropTarget) => {
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = "move";
    setDragOverTarget(target);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    // Only clear drag over target if we're leaving the drop zone entirely
    if (!e.currentTarget.contains(e.relatedTarget as Node)) {
      setDragOverTarget(null);
    }
  }, []);

  const handleDrop = useCallback(
    async (e: React.DragEvent, target: DropTarget) => {
      e.preventDefault();
      e.stopPropagation();

      try {
        const itemData = e.dataTransfer.getData("text/plain");
        const items: DragItem[] = JSON.parse(itemData);

        // Filter out invalid moves
        const validItems = items.filter((item) => {
          // Prevent dropping on itself
          if (item.id === target.id) {
            return false;
          }
          // Prevent dropping folder into itself
          if (item.type === "folder" && item.id === target.id) {
            return false;
          }
          return true;
        });

        if (validItems.length === 0) {
          toast.error("Cannot move items to this location");
          return;
        }

        // Update UI immediately for all items (optimistic update)
        if (onImmediateUpdate) {
          validItems.forEach((item) => {
            onImmediateUpdate(item.id, item.type, target.id);
          });
        }

        // Move all items in parallel
        const movePromises = validItems.map((item) => {
          if (item.type === "file") {
            return moveFile(item.id, { folderId: target.id });
          } else if (item.type === "folder") {
            return moveFolder(item.id, { parentId: target.id });
          }
          return Promise.resolve();
        });

        await Promise.all(movePromises);

        // Show success message
        if (validItems.length === 1) {
          toast.success(
            `${validItems[0].type === "folder" ? "Folder" : "File"} "${validItems[0].name}" moved to "${target.name}"`
          );
        } else {
          toast.success(`${validItems.length} items moved to "${target.name}"`);
        }

        // Refresh data after successful move with slight delay for smooth transition
        if (onRefresh) {
          setTimeout(async () => {
            await onRefresh();
          }, 150);
        }
      } catch (error) {
        console.error("Error moving items:", error);
        toast.error("Failed to move items");
        // Refresh to restore state on error
        if (onRefresh) {
          await onRefresh();
        }
      } finally {
        handleDragEnd();
      }
    },
    [onRefresh, handleDragEnd, onImmediateUpdate]
  );

  return {
    draggedItem,
    draggedItems,
    dragOverTarget,
    isDragging,
    handleDragStart,
    handleDragEnd,
    handleDragOver,
    handleDragLeave,
    handleDrop,
  };
}
