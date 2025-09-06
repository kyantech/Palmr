import { useEffect, useRef, useState } from "react";
import {
  IconArrowsMove,
  IconCheck,
  IconChevronDown,
  IconDotsVertical,
  IconDownload,
  IconEdit,
  IconEye,
  IconFolder,
  IconShare,
  IconTrash,
  IconX,
} from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { getFileIcon } from "@/utils/file-icons";
import { formatFileSize } from "@/utils/format-file-size";

interface File {
  id: string;
  name: string;
  description?: string;
  extension: string;
  size: number;
  objectName: string;
  userId: string;
  folderId?: string;
  createdAt: string;
  updatedAt: string;
}

interface Folder {
  id: string;
  name: string;
  description?: string;
  objectName: string;
  parentId?: string;
  userId: string;
  createdAt: string;
  updatedAt: string;
  totalSize?: string;
  _count?: {
    files: number;
    children: number;
  };
}

interface FilesTableProps {
  files: File[];
  folders?: Folder[];
  onPreview?: (file: File) => void;
  onRename?: (file: File) => void;
  onUpdateName?: (fileId: string, newName: string) => void;
  onUpdateDescription?: (fileId: string, newDescription: string) => void;
  onDownload: (objectName: string, fileName: string) => void;
  onShare?: (file: File) => void;
  onDelete?: (file: File) => void;
  onBulkDelete?: (files: File[], folders: Folder[]) => void;
  onBulkShare?: (files: File[], folders: Folder[]) => void;
  onBulkDownload?: (files: File[], folders: Folder[]) => void;
  onBulkMove?: (files: File[], folders: Folder[]) => void;
  setClearSelectionCallback?: (callback: () => void) => void;
  onNavigateToFolder?: (folderId: string) => void;
  onRenameFolder?: (folder: Folder) => void;
  onDeleteFolder?: (folder: Folder) => void;
  onShareFolder?: (folder: Folder) => void;
  onDownloadFolder?: (folderId: string, folderName: string) => Promise<void>;
  onMoveFolder?: (folder: Folder) => void;
  onMoveFile?: (file: File) => void;
  onUpdateFolderName?: (folderId: string, newName: string) => void;
  onUpdateFolderDescription?: (folderId: string, newDescription: string) => void;
  showBulkActions?: boolean;
  isShareMode?: boolean;
}

export function FilesTable({
  files,
  folders = [],
  onPreview,
  onRename,
  onUpdateName,
  onUpdateDescription,
  onDownload,
  onShare,
  onDelete,
  onBulkDelete,
  onBulkShare,
  onBulkDownload,
  onBulkMove,
  setClearSelectionCallback,
  onNavigateToFolder,
  onRenameFolder,
  onDeleteFolder,
  onShareFolder,
  onDownloadFolder,
  onMoveFolder,
  onMoveFile,
  onUpdateFolderName,
  onUpdateFolderDescription,
  showBulkActions = true,
  isShareMode = false,
}: FilesTableProps) {
  const t = useTranslations();
  const [editingField, setEditingField] = useState<{ fileId: string; field: "name" | "description" } | null>(null);
  const [editingFolderField, setEditingFolderField] = useState<{
    folderId: string;
    field: "name" | "description";
  } | null>(null);
  const [editValue, setEditValue] = useState("");
  const [hoveredField, setHoveredField] = useState<{ fileId: string; field: "name" | "description" } | null>(null);
  const [hoveredFolderField, setHoveredFolderField] = useState<{
    folderId: string;
    field: "name" | "description";
  } | null>(null);
  const [pendingChanges, setPendingChanges] = useState<{ [fileId: string]: { name?: string; description?: string } }>(
    {}
  );
  const [pendingFolderChanges, setPendingFolderChanges] = useState<{
    [folderId: string]: { name?: string; description?: string };
  }>({});

  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [selectedFolders, setSelectedFolders] = useState<Set<string>>(new Set());
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editingField && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingField]);

  useEffect(() => {
    setPendingChanges({});
  }, [files]);

  useEffect(() => {
    setPendingFolderChanges({});
  }, [folders]);

  const fileIds = files?.map((f) => f.id).join(",");
  useEffect(() => {
    setSelectedFiles(new Set());
  }, [fileIds]);

  const folderIds = folders?.map((f) => f.id).join(",");
  useEffect(() => {
    setSelectedFolders(new Set());
  }, [folderIds]);

  useEffect(() => {
    const clearSelection = () => {
      setSelectedFiles(new Set());
      setSelectedFolders(new Set());
    };
    setClearSelectionCallback?.(clearSelection);
  }, [setClearSelectionCallback]);

  const splitFileName = (fullName: string) => {
    const lastDotIndex = fullName.lastIndexOf(".");
    return lastDotIndex === -1
      ? { name: fullName, extension: "" }
      : {
          name: fullName.substring(0, lastDotIndex),
          extension: fullName.substring(lastDotIndex),
        };
  };

  const startEditFolder = (folderId: string, field: "name" | "description", currentValue: string) => {
    setEditingFolderField({ folderId, field });
    setEditValue(currentValue || "");
  };

  const saveEditFolder = () => {
    if (!editingFolderField) return;

    const { folderId, field } = editingFolderField;

    setPendingFolderChanges((prev) => ({
      ...prev,
      [folderId]: { ...prev[folderId], [field]: editValue },
    }));

    if (field === "name") {
      onUpdateFolderName?.(folderId, editValue);
    } else {
      onUpdateFolderDescription?.(folderId, editValue);
    }

    setEditingFolderField(null);
    setEditValue("");
    setHoveredFolderField(null);
  };

  const cancelEditFolder = () => {
    setEditingFolderField(null);
    setEditValue("");
    setHoveredFolderField(null);
  };

  const startEdit = (fileId: string, field: "name" | "description", currentValue: string) => {
    setEditingField({ fileId, field });
    if (field === "name") {
      const { name } = splitFileName(currentValue);
      setEditValue(name);
    } else {
      setEditValue(currentValue || "");
    }
  };

  const saveEdit = () => {
    if (!editingField) return;

    const { fileId, field } = editingField;
    if (field === "name") {
      const file = files.find((f) => f.id === fileId);
      if (file) {
        const { extension } = splitFileName(file.name);
        const newFullName = editValue + extension;

        setPendingChanges((prev) => ({
          ...prev,
          [fileId]: { ...prev[fileId], name: newFullName },
        }));

        onUpdateName?.(fileId, newFullName);
      }
    } else {
      setPendingChanges((prev) => ({
        ...prev,
        [fileId]: { ...prev[fileId], description: editValue },
      }));

      onUpdateDescription?.(fileId, editValue);
    }

    setEditingField(null);
    setEditValue("");
    setHoveredField(null);
  };

  const cancelEdit = () => {
    setEditingField(null);
    setEditValue("");
    setHoveredField(null);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      if (editingFolderField) {
        saveEditFolder();
      } else {
        saveEdit();
      }
    } else if (e.key === "Escape") {
      if (editingFolderField) {
        cancelEditFolder();
      } else {
        cancelEdit();
      }
    }
  };

  const formatDateTime = (dateString: string) => {
    const date = new Date(dateString);

    return new Intl.DateTimeFormat("en-US", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    }).format(date);
  };

  const getDisplayValue = (file: File, field: "name" | "description") => {
    const pendingChange = pendingChanges[file.id];
    if (pendingChange && pendingChange[field] !== undefined) {
      return pendingChange[field];
    }
    return field === "name" ? file.name : file.description;
  };

  const getDisplayFolderValue = (folder: Folder, field: "name" | "description") => {
    const pendingChange = pendingFolderChanges[folder.id];
    if (pendingChange && pendingChange[field] !== undefined) {
      return pendingChange[field];
    }
    return field === "name" ? folder.name : folder.description;
  };

  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedFiles(new Set(files.map((file) => file.id)));
      setSelectedFolders(new Set(folders.map((folder) => folder.id)));
    } else {
      setSelectedFiles(new Set());
      setSelectedFolders(new Set());
    }
  };

  const handleSelectFile = (fileId: string, checked: boolean) => {
    const newSelected = new Set(selectedFiles);
    if (checked) {
      newSelected.add(fileId);
    } else {
      newSelected.delete(fileId);
    }
    setSelectedFiles(newSelected);
  };

  const handleSelectFolder = (folderId: string, checked: boolean) => {
    const newSelected = new Set(selectedFolders);
    if (checked) {
      newSelected.add(folderId);
    } else {
      newSelected.delete(folderId);
    }
    setSelectedFolders(newSelected);
  };

  const getSelectedFiles = () => {
    return files.filter((file) => selectedFiles.has(file.id));
  };

  const getSelectedFolders = () => {
    return folders.filter((folder) => selectedFolders.has(folder.id));
  };

  const totalItems = files.length + folders.length;
  const selectedItems = selectedFiles.size + selectedFolders.size;
  const isAllSelected = totalItems > 0 && selectedItems === totalItems;

  const handleBulkAction = (action: "delete" | "share" | "download" | "move") => {
    const selectedFileObjects = getSelectedFiles();
    const selectedFolderObjects = getSelectedFolders();

    if (selectedFileObjects.length === 0 && selectedFolderObjects.length === 0) return;

    switch (action) {
      case "delete":
        if (onBulkDelete) {
          onBulkDelete(selectedFileObjects, selectedFolderObjects);
        }
        break;
      case "share":
        if (onBulkShare) {
          onBulkShare(selectedFileObjects, selectedFolderObjects);
        }
        break;
      case "download":
        if (onBulkDownload) {
          onBulkDownload(selectedFileObjects, selectedFolderObjects);
        }
        break;
      case "move":
        if (onBulkMove) {
          onBulkMove(selectedFileObjects, selectedFolderObjects);
        }
        break;
    }
  };

  const shouldShowBulkActions =
    showBulkActions &&
    (selectedFiles.size > 0 || selectedFolders.size > 0) &&
    (isShareMode ? onBulkDownload : onBulkDelete || onBulkShare || onBulkDownload || onBulkMove);

  return (
    <div className="space-y-4">
      {shouldShowBulkActions && (
        <div className="flex items-center justify-between p-4 bg-muted/30 border rounded-lg">
          <div className="flex items-center gap-3">
            <span className="text-sm font-medium text-foreground">
              {t("filesTable.bulkActions.selected", { count: selectedFiles.size + selectedFolders.size })}
            </span>
          </div>
          <div className="flex items-center gap-2">
            {isShareMode ? (
              onBulkDownload && (
                <Button variant="default" size="sm" className="gap-2" onClick={() => handleBulkAction("download")}>
                  <IconDownload className="h-4 w-4" />
                  {t("filesTable.bulkActions.download")}
                </Button>
              )
            ) : (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="default" size="sm" className="gap-2">
                    {t("filesTable.bulkActions.actions")}
                    <IconChevronDown className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-[200px]">
                  {onBulkMove && (
                    <DropdownMenuItem className="cursor-pointer py-2" onClick={() => handleBulkAction("move")}>
                      <IconArrowsMove className="h-4 w-4" />
                      {t("common.move")}
                    </DropdownMenuItem>
                  )}
                  {onBulkDownload && (
                    <DropdownMenuItem className="cursor-pointer py-2" onClick={() => handleBulkAction("download")}>
                      <IconDownload className="h-4 w-4" />
                      {t("filesTable.bulkActions.download")}
                    </DropdownMenuItem>
                  )}
                  {onBulkShare && (
                    <DropdownMenuItem className="cursor-pointer py-2" onClick={() => handleBulkAction("share")}>
                      <IconShare className="h-4 w-4" />
                      {t("filesTable.bulkActions.share")}
                    </DropdownMenuItem>
                  )}
                  {onBulkDelete && (
                    <DropdownMenuItem
                      onClick={() => handleBulkAction("delete")}
                      className="cursor-pointer py-2 text-destructive focus:text-destructive"
                    >
                      <IconTrash className="h-4 w-4" />
                      {t("filesTable.bulkActions.delete")}
                    </DropdownMenuItem>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setSelectedFiles(new Set());
                setSelectedFolders(new Set());
              }}
            >
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      )}

      <div className="rounded-lg shadow-sm overflow-hidden border">
        <Table>
          <TableHeader>
            <TableRow className="border-b-0">
              {showBulkActions && (
                <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4 w-12">
                  <Checkbox
                    checked={isAllSelected}
                    onCheckedChange={handleSelectAll}
                    aria-label={t("filesTable.selectAll")}
                  />
                </TableHead>
              )}
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.name")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.description")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.size")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.createdAt")}
              </TableHead>
              <TableHead className="h-10 text-xs font-bold text-muted-foreground bg-muted/50 px-4">
                {t("filesTable.columns.updatedAt")}
              </TableHead>
              <TableHead className="h-10 w-[70px] text-xs font-bold text-muted-foreground bg-muted/50 px-4 rounded-tr-lg">
                {t("filesTable.columns.actions")}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {folders.map((folder) => {
              const isSelected = selectedFolders.has(folder.id);
              const isEditingName = editingFolderField?.folderId === folder.id && editingFolderField?.field === "name";
              const isEditingDescription =
                editingFolderField?.folderId === folder.id && editingFolderField?.field === "description";
              const isHoveringName = hoveredFolderField?.folderId === folder.id && hoveredFolderField?.field === "name";
              const isHoveringDescriptionField =
                hoveredFolderField?.folderId === folder.id && hoveredFolderField?.field === "description";

              const displayName = getDisplayFolderValue(folder, "name") || folder.name;
              const displayDescription = getDisplayFolderValue(folder, "description");

              return (
                <TableRow key={folder.id} className="group hover:bg-muted/50 transition-colors border-0">
                  {showBulkActions && (
                    <TableCell className="h-12 px-4 border-0">
                      <Checkbox
                        checked={isSelected}
                        onCheckedChange={(checked: boolean) => handleSelectFolder(folder.id, checked)}
                        aria-label={`Select folder ${folder.name}`}
                      />
                    </TableCell>
                  )}
                  <TableCell className="h-12 px-4 border-0">
                    <div className="flex items-center gap-2">
                      <div
                        className="flex items-center gap-2 cursor-pointer hover:opacity-80 transition-opacity"
                        onClick={(e) => {
                          e.stopPropagation();
                          onNavigateToFolder?.(folder.id);
                        }}
                        onMouseEnter={() => setHoveredFolderField({ folderId: folder.id, field: "name" })}
                        onMouseLeave={() => setHoveredFolderField(null)}
                      >
                        <IconFolder className="h-5.5 w-5.5 text-primary" />
                        <div className="flex items-center gap-1 min-w-0 flex-1">
                          {isEditingName ? (
                            <div className="flex items-center gap-1 flex-1">
                              <Input
                                ref={inputRef}
                                value={editValue}
                                onChange={(e) => setEditValue(e.target.value)}
                                onKeyDown={handleKeyDown}
                                className="h-8 text-sm font-medium"
                                onClick={(e) => e.stopPropagation()}
                              />
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-green-600 hover:text-green-700 flex-shrink-0"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  saveEditFolder();
                                }}
                              >
                                <IconCheck className="h-3 w-3" />
                              </Button>
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-red-600 hover:text-red-700 flex-shrink-0"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  cancelEditFolder();
                                }}
                              >
                                <IconX className="h-3 w-3" />
                              </Button>
                            </div>
                          ) : (
                            <div className="flex items-center gap-1 flex-1">
                              <span
                                className="font-medium text-sm text-foreground/90 truncate max-w-[150px]"
                                title={displayName}
                              >
                                {displayName}
                              </span>
                              <div className="w-6 flex justify-center flex-shrink-0">
                                {isHoveringName && !isShareMode && (
                                  <Button
                                    size="icon"
                                    variant="ghost"
                                    className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      startEditFolder(folder.id, "name", folder.name);
                                    }}
                                  >
                                    <IconEdit className="h-3 w-3" />
                                  </Button>
                                )}
                              </div>
                            </div>
                          )}
                        </div>
                      </div>
                    </div>
                  </TableCell>
                  <TableCell
                    className="h-12 px-4"
                    onMouseEnter={() => setHoveredFolderField({ folderId: folder.id, field: "description" })}
                    onMouseLeave={() => setHoveredFolderField(null)}
                  >
                    <div className="flex items-center gap-1">
                      {isEditingDescription ? (
                        <div className="flex items-center gap-1 flex-1">
                          <Input
                            ref={inputRef}
                            value={editValue}
                            onChange={(e) => setEditValue(e.target.value)}
                            onKeyDown={handleKeyDown}
                            className="h-8 text-sm"
                            onClick={(e) => e.stopPropagation()}
                          />
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-green-600 hover:text-green-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              saveEditFolder();
                            }}
                          >
                            <IconCheck className="h-3 w-3" />
                          </Button>
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-red-600 hover:text-red-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              cancelEditFolder();
                            }}
                          >
                            <IconX className="h-3 w-3" />
                          </Button>
                        </div>
                      ) : (
                        <div className="flex items-center gap-1 flex-1 min-w-0">
                          <span
                            className="text-muted-foreground truncate max-w-[150px]"
                            title={displayDescription || "-"}
                          >
                            {displayDescription || "-"}
                          </span>
                          <div className="w-6 flex justify-center flex-shrink-0">
                            {isHoveringDescriptionField && !isShareMode && (
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  startEditFolder(folder.id, "description", folder.description || "");
                                }}
                              >
                                <IconEdit className="h-3 w-3" />
                              </Button>
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">
                    {folder.totalSize ? formatFileSize(Number(folder.totalSize)) : "—"}
                  </TableCell>
                  <TableCell className="h-12 px-4">{formatDateTime(folder.createdAt)}</TableCell>
                  <TableCell className="h-12 px-4">{formatDateTime(folder.updatedAt)}</TableCell>
                  <TableCell className="h-12 px-4 text-right">
                    {isShareMode ? (
                      onDownloadFolder && (
                        <Button
                          size="icon"
                          variant="ghost"
                          className="h-8 w-8 hover:bg-muted"
                          onClick={() => onDownloadFolder(folder.id, folder.name)}
                        >
                          <IconDownload className="h-4 w-4" />
                          <span className="sr-only">{t("filesTable.actions.download")}</span>
                        </Button>
                      )
                    ) : (
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button variant="ghost" size="icon" className="h-8 w-8 hover:bg-muted cursor-pointer">
                            <IconDotsVertical className="h-4 w-4" />
                            <span className="sr-only">Folder actions menu</span>
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="w-[200px]">
                          {onRenameFolder && (
                            <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onRenameFolder(folder)}>
                              <IconEdit className="h-4 w-4" />
                              {t("filesTable.actions.edit")}
                            </DropdownMenuItem>
                          )}
                          {onMoveFolder && (
                            <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onMoveFolder(folder)}>
                              <IconArrowsMove className="h-4 w-4" />
                              Move
                            </DropdownMenuItem>
                          )}
                          {onDownloadFolder && (
                            <DropdownMenuItem
                              className="cursor-pointer py-2"
                              onClick={() => onDownloadFolder(folder.id, folder.name)}
                            >
                              <IconDownload className="h-4 w-4" />
                              {t("filesTable.actions.download")}
                            </DropdownMenuItem>
                          )}
                          {onShareFolder && (
                            <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onShareFolder(folder)}>
                              <IconShare className="h-4 w-4" />
                              {t("filesTable.actions.share")}
                            </DropdownMenuItem>
                          )}
                          {onDeleteFolder && (
                            <DropdownMenuItem
                              onClick={() => onDeleteFolder(folder)}
                              className="cursor-pointer py-2 text-destructive focus:text-destructive"
                            >
                              <IconTrash className="h-4 w-4" />
                              {t("filesTable.actions.delete")}
                            </DropdownMenuItem>
                          )}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    )}
                  </TableCell>
                </TableRow>
              );
            })}
            {files.map((file) => {
              const { icon: FileIcon, color } = getFileIcon(file.name);
              const isEditingName = editingField?.fileId === file.id && editingField?.field === "name";
              const isEditingDescription = editingField?.fileId === file.id && editingField?.field === "description";
              const isHoveringName = hoveredField?.fileId === file.id && hoveredField?.field === "name";
              const isHoveringDescription = hoveredField?.fileId === file.id && hoveredField?.field === "description";
              const isSelected = selectedFiles.has(file.id);

              const displayName = getDisplayValue(file, "name") || file.name;
              const displayDescription = getDisplayValue(file, "description");

              return (
                <TableRow
                  key={file.id}
                  className="group hover:bg-muted/50 transition-colors border-0 cursor-pointer"
                  onClick={(e) => {
                    // Don't trigger preview if clicking on checkbox, buttons, or dropdown
                    if (
                      (e.target as HTMLElement).closest(".checkbox-wrapper") ||
                      (e.target as HTMLElement).closest("button") ||
                      (e.target as HTMLElement).closest('[role="menuitem"]')
                    ) {
                      return;
                    }
                    if (onPreview) {
                      onPreview(file);
                    }
                  }}
                >
                  {showBulkActions && (
                    <TableCell className="h-12 px-4 border-0">
                      <div className="checkbox-wrapper">
                        <Checkbox
                          checked={isSelected}
                          onCheckedChange={(checked: boolean) => handleSelectFile(file.id, checked)}
                          aria-label={t("filesTable.selectFile", { fileName: file.name })}
                        />
                      </div>
                    </TableCell>
                  )}
                  <TableCell className="h-12 px-4 border-0">
                    <div className="flex items-center gap-2">
                      <div
                        className="flex items-center gap-2 cursor-pointer hover:opacity-80 transition-opacity"
                        onClick={(e) => {
                          e.stopPropagation();
                          onPreview?.(file);
                        }}
                        onMouseEnter={() => setHoveredField({ fileId: file.id, field: "name" })}
                        onMouseLeave={() => setHoveredField(null)}
                      >
                        <FileIcon className={`h-5.5 w-5.5 ${color}`} />
                        <div className="flex items-center gap-1 min-w-0 flex-1">
                          {isEditingName ? (
                            <div className="flex items-center gap-1 flex-1">
                              <Input
                                ref={inputRef}
                                value={editValue}
                                onChange={(e) => setEditValue(e.target.value)}
                                onKeyDown={handleKeyDown}
                                className="h-8 text-sm font-medium"
                                onClick={(e) => e.stopPropagation()}
                              />
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-green-600 hover:text-green-700 flex-shrink-0"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  saveEdit();
                                }}
                              >
                                <IconCheck className="h-3 w-3" />
                              </Button>
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-red-600 hover:text-red-700 flex-shrink-0"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  cancelEdit();
                                }}
                              >
                                <IconX className="h-3 w-3" />
                              </Button>
                            </div>
                          ) : (
                            <div className="flex items-center gap-1 flex-1 min-w-0">
                              <span className="truncate max-w-[200px] font-medium" title={displayName}>
                                {displayName}
                              </span>
                              <div className="w-6 flex justify-center flex-shrink-0">
                                {isHoveringName && !isShareMode && (
                                  <Button
                                    size="icon"
                                    variant="ghost"
                                    className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      startEdit(file.id, "name", displayName);
                                    }}
                                  >
                                    <IconEdit className="h-3 w-3" />
                                  </Button>
                                )}
                              </div>
                            </div>
                          )}
                        </div>
                      </div>
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">
                    <div
                      className="flex items-center gap-1"
                      onMouseEnter={() => setHoveredField({ fileId: file.id, field: "description" })}
                      onMouseLeave={() => setHoveredField(null)}
                    >
                      {isEditingDescription ? (
                        <div className="flex items-center gap-1 flex-1">
                          <Input
                            ref={inputRef}
                            value={editValue}
                            onChange={(e) => setEditValue(e.target.value)}
                            onKeyDown={handleKeyDown}
                            placeholder={t("fileActions.addDescriptionPlaceholder")}
                            className="h-8 text-sm"
                            onClick={(e) => e.stopPropagation()}
                          />
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-green-600 hover:text-green-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              saveEdit();
                            }}
                          >
                            <IconCheck className="h-3 w-3" />
                          </Button>
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-6 w-6 text-red-600 hover:text-red-700 flex-shrink-0"
                            onClick={(e) => {
                              e.stopPropagation();
                              cancelEdit();
                            }}
                          >
                            <IconX className="h-3 w-3" />
                          </Button>
                        </div>
                      ) : (
                        <div className="flex items-center gap-1 flex-1 min-w-0">
                          <span
                            className="text-muted-foreground truncate max-w-[150px]"
                            title={displayDescription || "-"}
                          >
                            {displayDescription || "-"}
                          </span>
                          <div className="w-6 flex justify-center flex-shrink-0">
                            {isHoveringDescription && !isShareMode && (
                              <Button
                                size="icon"
                                variant="ghost"
                                className="h-6 w-6 text-muted-foreground hover:text-foreground hidden sm:block"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  startEdit(file.id, "description", displayDescription || "");
                                }}
                              >
                                <IconEdit className="h-3 w-3" />
                              </Button>
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className="h-12 px-4">{formatFileSize(file.size)}</TableCell>
                  <TableCell className="h-12 px-4">{formatDateTime(file.createdAt)}</TableCell>
                  <TableCell className="h-12 px-4">{formatDateTime(file.updatedAt || file.createdAt)}</TableCell>
                  <TableCell className="h-12 px-4 text-right">
                    {isShareMode ? (
                      <Button
                        size="icon"
                        variant="ghost"
                        className="h-8 w-8 hover:bg-muted"
                        onClick={() => onDownload(file.objectName, file.name)}
                      >
                        <IconDownload className="h-4 w-4" />
                        <span className="sr-only">{t("filesTable.actions.download")}</span>
                      </Button>
                    ) : (
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button variant="ghost" size="icon" className="h-8 w-8 hover:bg-muted cursor-pointer">
                            <IconDotsVertical className="h-4 w-4" />
                            <span className="sr-only">{t("filesTable.actions.menu")}</span>
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="w-[200px]">
                          {onPreview && (
                            <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onPreview(file)}>
                              <IconEye className="h-4 w-4" />
                              {t("filesTable.actions.preview")}
                            </DropdownMenuItem>
                          )}
                          {onRename && (
                            <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onRename(file)}>
                              <IconEdit className="h-4 w-4" />
                              {t("filesTable.actions.edit")}
                            </DropdownMenuItem>
                          )}
                          {onMoveFile && (
                            <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onMoveFile(file)}>
                              <IconArrowsMove className="h-4 w-4" />
                              {t("common.move")}
                            </DropdownMenuItem>
                          )}
                          <DropdownMenuItem
                            className="cursor-pointer py-2"
                            onClick={() => onDownload(file.objectName, file.name)}
                          >
                            <IconDownload className="h-4 w-4" />
                            {t("filesTable.actions.download")}
                          </DropdownMenuItem>
                          {onShare && (
                            <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onShare(file)}>
                              <IconShare className="h-4 w-4" />
                              {t("filesTable.actions.share")}
                            </DropdownMenuItem>
                          )}
                          {onDelete && (
                            <DropdownMenuItem
                              onClick={() => onDelete(file)}
                              className="cursor-pointer py-2 text-destructive focus:text-destructive"
                            >
                              <IconTrash className="h-4 w-4" />
                              {t("filesTable.actions.delete")}
                            </DropdownMenuItem>
                          )}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    )}
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
