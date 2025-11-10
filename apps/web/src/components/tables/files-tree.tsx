"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { IconChevronDown, IconChevronRight, IconFolder, IconFolderOpen } from "@tabler/icons-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { FileItem } from "@/http/endpoints/files/types";
import type { FolderItem } from "@/http/endpoints/folders/types";
import { cn } from "@/lib/utils";
import { getFileIcon } from "@/utils/file-icons";

export interface TreeFile {
  id: string;
  name: string;
  type: "file";
  size?: number;
  parentId: string | null;
}

export interface TreeFolder {
  id: string;
  name: string;
  type: "folder";
  parentId: string | null;
  totalSize?: string;
}

export type TreeItem = TreeFile | TreeFolder;

export interface FileTreeProps {
  files: FileItem[];
  folders: FolderItem[];
  selectedItems: string[];
  onSelectionChange: (selectedIds: string[]) => void;
  showFiles?: boolean;
  showFolders?: boolean;
  className?: string;
  maxHeight?: string;
  singleSelection?: boolean;
  useRadioButtons?: boolean;
  useCheckboxAsRadio?: boolean;
  searchQuery?: string;
  autoExpandToItem?: string | null;
}

interface TreeNode {
  item: TreeItem;
  children: TreeNode[];
  level: number;
}

interface TreeNodeProps {
  node: TreeNode;
  isExpanded: boolean;
  isSelected: boolean;
  isIndeterminate: boolean;
  onToggleExpand: (nodeId: string) => void;
  onToggleSelect: (nodeId: string) => void;
  expandedFolders: Set<string>;
  selectedSet: Set<string>;
  showFiles: boolean;
  showFolders: boolean;
  singleSelection?: boolean;
  useRadioButtons?: boolean;
  useCheckboxAsRadio?: boolean;
}

function formatFileSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function TreeNodeComponent({
  node,
  isExpanded,
  isSelected,
  isIndeterminate,
  onToggleExpand,
  onToggleSelect,
  expandedFolders,
  selectedSet,
  showFiles,
  showFolders,
  singleSelection = false,
  useRadioButtons = false,
  useCheckboxAsRadio = false,
}: TreeNodeProps) {
  const { item, children, level } = node;
  const isFolder = item.type === "folder";
  const hasChildren = children.length > 0;
  const shouldShow = (isFolder && showFolders) || (!isFolder && showFiles);

  if (!shouldShow) return null;

  const paddingLeft = level * 20 + 8;

  return (
    <div>
      <div
        className={cn(
          "flex items-center gap-2 py-1 px-2 hover:bg-muted/50 rounded-sm w-full min-w-0",
          isSelected && "bg-muted"
        )}
        style={{ paddingLeft }}
      >
        <div className="w-4 h-4 flex items-center justify-center flex-shrink-0">
          {isFolder && hasChildren && (
            <Button variant="ghost" size="sm" className="h-4 w-4 p-0" onClick={() => onToggleExpand(item.id)}>
              {isExpanded ? <IconChevronDown className="h-3 w-3" /> : <IconChevronRight className="h-3 w-3" />}
            </Button>
          )}
        </div>

        {useRadioButtons ? (
          <input
            type="radio"
            name="tree-selection"
            checked={isSelected}
            onChange={() => onToggleSelect(item.id)}
            className="flex-shrink-0"
          />
        ) : (
          <Checkbox
            checked={isSelected}
            ref={(ref) => {
              if (ref && ref.querySelector) {
                const checkbox = ref.querySelector('input[type="checkbox"]') as HTMLInputElement;
                if (checkbox && !useCheckboxAsRadio) {
                  checkbox.indeterminate = isIndeterminate;
                }
              }
            }}
            onCheckedChange={() => onToggleSelect(item.id)}
            className="flex-shrink-0"
          />
        )}

        <div className="flex items-center gap-2 flex-1 min-w-0 overflow-hidden">
          {isFolder ? (
            isExpanded ? (
              <IconFolderOpen className="h-4 w-4 flex-shrink-0 text-primary" />
            ) : (
              <IconFolder className="h-4 w-4 flex-shrink-0 text-primary" />
            )
          ) : (
            (() => {
              const { icon: FileIcon, color } = getFileIcon(item.name);
              return <FileIcon className={`h-4 w-4 flex-shrink-0 ${color}`} />;
            })()
          )}

          <span className="truncate text-sm">{item.name}</span>

          {!isFolder && item.size && (
            <span className="text-xs text-muted-foreground flex-shrink-0">({formatFileSize(item.size)})</span>
          )}

          {isFolder && (item as TreeFolder).totalSize && (
            <span className="text-xs text-muted-foreground flex-shrink-0">
              ({formatFileSize(Number((item as TreeFolder).totalSize!))})
            </span>
          )}
        </div>
      </div>

      {isFolder && isExpanded && hasChildren && (
        <div>
          {children.map((childNode) => (
            <TreeNodeComponent
              key={childNode.item.id}
              node={childNode}
              isExpanded={expandedFolders.has(childNode.item.id)}
              isSelected={selectedSet.has(childNode.item.id)}
              isIndeterminate={false}
              onToggleExpand={onToggleExpand}
              onToggleSelect={onToggleSelect}
              expandedFolders={expandedFolders}
              selectedSet={selectedSet}
              showFiles={showFiles}
              showFolders={showFolders}
              singleSelection={singleSelection}
              useRadioButtons={useRadioButtons}
              useCheckboxAsRadio={useCheckboxAsRadio}
            />
          ))}
        </div>
      )}
    </div>
  );
}

export function FileTree({
  files,
  folders,
  selectedItems,
  onSelectionChange,
  showFiles = true,
  showFolders = true,
  className,
  maxHeight = "400px",
  singleSelection = false,
  useRadioButtons = false,
  useCheckboxAsRadio = false,
  searchQuery = "",
  autoExpandToItem = null,
}: FileTreeProps) {
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set());
  const selectedSet = useMemo(() => new Set(selectedItems), [selectedItems]);

  const convertToTreeItems = useCallback((): TreeItem[] => {
    let treeFolders: TreeItem[] = folders.map((folder) => ({
      id: folder.id,
      name: folder.name,
      type: "folder" as const,
      parentId: folder.parentId,
      totalSize: folder.totalSize,
    }));

    let treeFiles: TreeItem[] = files.map((file) => ({
      id: file.id,
      name: file.name,
      type: "file" as const,
      size: parseInt(file.size),
      parentId: file.folderId,
    }));

    if (searchQuery.trim()) {
      const searchLower = searchQuery.toLowerCase();

      const getMatchingItems = (allItems: TreeItem[]): TreeItem[] => {
        const matching = new Set<string>();

        allItems.forEach((item) => {
          if (item.name.toLowerCase().includes(searchLower)) {
            matching.add(item.id);
          }
        });

        const addParents = (itemId: string) => {
          const item = allItems.find((i) => i.id === itemId);
          if (item && item.parentId) {
            matching.add(item.parentId);
            addParents(item.parentId);
          }
        };

        matching.forEach((itemId) => addParents(itemId));

        return allItems.filter((item) => matching.has(item.id));
      };

      const allItems = [...treeFolders, ...treeFiles];
      const filteredItems = getMatchingItems(allItems);

      treeFolders = filteredItems.filter((item) => item.type === "folder") as TreeFolder[];
      treeFiles = filteredItems.filter((item) => item.type === "file") as TreeFile[];
    }

    return [...treeFolders, ...treeFiles];
  }, [files, folders, searchQuery]);

  useEffect(() => {
    if (autoExpandToItem) {
      const allItems = convertToTreeItems();
      const item = allItems.find((i) => i.id === autoExpandToItem);
      if (item && item.parentId) {
        const pathToRoot: string[] = [];
        let currentItem: TreeItem | undefined = item;

        while (currentItem && currentItem.parentId) {
          pathToRoot.push(currentItem.parentId);
          currentItem = allItems.find((i) => i.id === currentItem!.parentId);
        }

        if (pathToRoot.length > 0) {
          setExpandedFolders(new Set(pathToRoot));
        }
      }
    }
  }, [autoExpandToItem, convertToTreeItems]);

  const tree = useMemo(() => {
    const allItems = convertToTreeItems();
    const itemMap = new Map<string, TreeItem>();
    const childrenMap = new Map<string, TreeItem[]>();

    allItems.forEach((item) => {
      itemMap.set(item.id, item);
      childrenMap.set(item.id, []);
    });

    allItems.forEach((item) => {
      if (item.parentId) {
        const parentChildren = childrenMap.get(item.parentId);
        if (parentChildren) {
          parentChildren.push(item);
        }
      }
    });

    function buildTreeNode(item: TreeItem, level: number): TreeNode {
      const children = childrenMap.get(item.id) || [];
      return {
        item,
        children: children
          .sort((a, b) => {
            if (a.type !== b.type) {
              return a.type === "folder" ? -1 : 1;
            }
            return a.name.localeCompare(b.name);
          })
          .map((child) => buildTreeNode(child, level + 1)),
        level,
      };
    }

    const rootItems = allItems.filter((item) => !item.parentId);

    return rootItems
      .sort((a, b) => {
        if (a.type !== b.type) {
          return a.type === "folder" ? -1 : 1;
        }
        return a.name.localeCompare(b.name);
      })
      .map((item) => buildTreeNode(item, 0));
  }, [convertToTreeItems]);

  const getDescendants = useCallback(
    (folderId: string): string[] => {
      const descendants: string[] = [];
      const allItems = convertToTreeItems();

      function collectDescendants(parentId: string) {
        allItems.forEach((item) => {
          if (item.parentId === parentId) {
            descendants.push(item.id);
            if (item.type === "folder") {
              collectDescendants(item.id);
            }
          }
        });
      }

      collectDescendants(folderId);
      return descendants;
    },
    [convertToTreeItems]
  );

  const getAllAncestors = useCallback(
    (itemId: string): string[] => {
      const ancestors: string[] = [];
      const allItems = convertToTreeItems();
      let currentItem = allItems.find((i) => i.id === itemId);

      while (currentItem && currentItem.parentId) {
        ancestors.push(currentItem.parentId);
        currentItem = allItems.find((i) => i.id === currentItem!.parentId);
      }

      return ancestors;
    },
    [convertToTreeItems]
  );

  const handleToggleExpand = useCallback((folderId: string) => {
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(folderId)) {
        next.delete(folderId);
      } else {
        next.add(folderId);
      }
      return next;
    });
  }, []);

  const handleToggleSelect = useCallback(
    (itemId: string) => {
      const allItems = convertToTreeItems();
      const item = allItems.find((i) => i.id === itemId);
      if (!item) return;

      if (singleSelection || useCheckboxAsRadio) {
        onSelectionChange([itemId]);
        return;
      }

      const newSelection = new Set(selectedItems);

      if (selectedSet.has(itemId)) {
        newSelection.delete(itemId);

        if (item.type === "folder") {
          const descendants = getDescendants(itemId);
          descendants.forEach((id) => newSelection.delete(id));
        } else {
          const ancestors = getAllAncestors(itemId);
          const allItems = convertToTreeItems();

          ancestors.forEach((ancestorId) => {
            const ancestorDescendants = getDescendants(ancestorId);

            const selectedDescendants = ancestorDescendants.filter((id) => id !== itemId && newSelection.has(id));

            if (selectedDescendants.length === 0) {
              const ancestorSiblings = allItems.filter((i) => {
                const ancestor = allItems.find((a) => a.id === ancestorId);
                return ancestor && i.parentId === ancestor.parentId && i.id !== ancestorId;
              });

              const selectedSiblings = ancestorSiblings.filter((sibling) => newSelection.has(sibling.id));

              if (selectedSiblings.length === 0) {
                newSelection.delete(ancestorId);
              }
            }
          });
        }
      } else {
        newSelection.add(itemId);

        const ancestors = getAllAncestors(itemId);
        ancestors.forEach((ancestorId) => {
          newSelection.add(ancestorId);
        });

        if (item.type === "folder") {
          const descendants = getDescendants(itemId);
          const allItems = convertToTreeItems();
          descendants.forEach((id) => {
            const descendantItem = allItems.find((i) => i.id === id);
            if (descendantItem) {
              if ((descendantItem.type === "folder" && showFolders) || (descendantItem.type === "file" && showFiles)) {
                newSelection.add(id);
              }
            }
          });
        }
      }

      onSelectionChange(Array.from(newSelection));
    },
    [
      selectedItems,
      selectedSet,
      getDescendants,
      getAllAncestors,
      onSelectionChange,
      showFiles,
      showFolders,
      convertToTreeItems,
      singleSelection,
      useCheckboxAsRadio,
    ]
  );

  const isIndeterminate = useCallback(
    (folderId: string): boolean => {
      const descendants = getDescendants(folderId);
      const allItems = convertToTreeItems();
      const visibleDescendants = descendants.filter((id) => {
        const item = allItems.find((i) => i.id === id);
        if (!item) return false;
        return (item.type === "folder" && showFolders) || (item.type === "file" && showFiles);
      });

      if (visibleDescendants.length === 0) return false;

      const selectedDescendants = visibleDescendants.filter((id) => selectedSet.has(id));
      return selectedDescendants.length > 0 && selectedDescendants.length < visibleDescendants.length;
    },
    [getDescendants, selectedSet, showFiles, showFolders, convertToTreeItems]
  );

  if (tree.length === 0) {
    return (
      <div className={cn("flex items-center justify-center py-8 text-muted-foreground", className)}>
        <p>No items to display</p>
      </div>
    );
  }

  return (
    <div className={cn("border rounded-md w-full", className)}>
      <div className="overflow-auto p-2 w-full" style={{ maxHeight }}>
        {tree.map((node) => (
          <TreeNodeComponent
            key={node.item.id}
            node={node}
            isExpanded={expandedFolders.has(node.item.id)}
            isSelected={selectedSet.has(node.item.id)}
            isIndeterminate={node.item.type === "folder" ? isIndeterminate(node.item.id) : false}
            onToggleExpand={handleToggleExpand}
            onToggleSelect={handleToggleSelect}
            expandedFolders={expandedFolders}
            selectedSet={selectedSet}
            showFiles={showFiles}
            showFolders={showFolders}
            singleSelection={singleSelection}
            useRadioButtons={useRadioButtons}
            useCheckboxAsRadio={useCheckboxAsRadio}
          />
        ))}
      </div>
    </div>
  );
}
