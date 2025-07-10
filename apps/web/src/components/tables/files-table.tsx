import { useEffect, useRef, useState } from "react";
import {
  IconCheck,
  IconChevronDown,
  IconDotsVertical,
  IconDownload,
  IconEdit,
  IconEye,
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
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface FilesTableProps {
  files: File[];
  onPreview: (file: File) => void;
  onRename: (file: File) => void;
  onUpdateName: (fileId: string, newName: string) => void;
  onUpdateDescription: (fileId: string, newDescription: string) => void;
  onDownload: (objectName: string, fileName: string) => void;
  onShare: (file: File) => void;
  onDelete: (file: File) => void;
  onBulkDelete?: (files: File[]) => void;
  onBulkShare?: (files: File[]) => void;
  onBulkDownload?: (files: File[]) => void;
  setClearSelectionCallback?: (callback: () => void) => void;
}

export function FilesTable({
  files,
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
  setClearSelectionCallback,
}: FilesTableProps) {
  const t = useTranslations();
  const [editingField, setEditingField] = useState<{ fileId: string; field: "name" | "description" } | null>(null);
  const [editValue, setEditValue] = useState("");
  const [hoveredField, setHoveredField] = useState<{ fileId: string; field: "name" | "description" } | null>(null);
  const [pendingChanges, setPendingChanges] = useState<{ [fileId: string]: { name?: string; description?: string } }>(
    {}
  );
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
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
    setSelectedFiles(new Set());
  }, [files]);

  useEffect(() => {
    const clearSelection = () => setSelectedFiles(new Set());
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

        onUpdateName(fileId, newFullName);
      }
    } else {
      setPendingChanges((prev) => ({
        ...prev,
        [fileId]: { ...prev[fileId], description: editValue },
      }));

      onUpdateDescription(fileId, editValue);
    }

    setEditingField(null);
    setEditValue("");
  };

  const cancelEdit = () => {
    setEditingField(null);
    setEditValue("");
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      saveEdit();
    } else if (e.key === "Escape") {
      cancelEdit();
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

  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedFiles(new Set(files.map((file) => file.id)));
    } else {
      setSelectedFiles(new Set());
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

  const getSelectedFiles = () => {
    return files.filter((file) => selectedFiles.has(file.id));
  };

  const isAllSelected = files.length > 0 && selectedFiles.size === files.length;

  const handleBulkAction = (action: "delete" | "share" | "download") => {
    const selectedFileObjects = getSelectedFiles();

    if (selectedFileObjects.length === 0) return;

    switch (action) {
      case "delete":
        if (onBulkDelete) {
          onBulkDelete(selectedFileObjects);
        }
        break;
      case "share":
        if (onBulkShare) {
          onBulkShare(selectedFileObjects);
        }
        break;
      case "download":
        if (onBulkDownload) {
          onBulkDownload(selectedFileObjects);
        }
        break;
    }
  };

  const showBulkActions = selectedFiles.size > 0 && (onBulkDelete || onBulkShare || onBulkDownload);

  return (
    <div className="space-y-4">
      {showBulkActions && (
        <div className="flex items-center justify-between p-4 bg-muted/30 border rounded-lg">
          <div className="flex items-center gap-3">
            <span className="text-sm font-medium text-foreground">
              {t("filesTable.bulkActions.selected", { count: selectedFiles.size })}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="default" size="sm" className="gap-2">
                  {t("filesTable.bulkActions.actions")}
                  <IconChevronDown className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-[200px]">
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
            <Button variant="outline" size="sm" onClick={() => setSelectedFiles(new Set())}>
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      )}

      <div className="rounded-lg shadow-sm overflow-hidden border">
        <Table>
          <TableHeader>
            <TableRow className="border-b-0">
              <TableHead className="h-10 w-[50px] text-xs font-bold text-muted-foreground bg-muted/50 px-4 rounded-tl-lg">
                <Checkbox
                  checked={isAllSelected}
                  onCheckedChange={handleSelectAll}
                  aria-label={t("filesTable.selectAll")}
                />
              </TableHead>
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
                <TableRow key={file.id} className="hover:bg-muted/50 transition-colors border-0">
                  <TableCell className="h-12 px-4 border-0">
                    <Checkbox
                      checked={isSelected}
                      onCheckedChange={(checked: boolean) => handleSelectFile(file.id, checked)}
                      aria-label={t("filesTable.selectFile", { fileName: file.name })}
                    />
                  </TableCell>
                  <TableCell className="h-12 px-4 border-0">
                    <div className="flex items-center gap-2">
                      <FileIcon
                        className={`h-5.5 w-5.5 ${color} cursor-pointer hover:opacity-80 transition-opacity`}
                        onClick={(e) => {
                          e.stopPropagation();
                          onPreview(file);
                        }}
                      />
                      <div
                        className="flex items-center gap-1 min-w-0 flex-1"
                        onMouseEnter={() => setHoveredField({ fileId: file.id, field: "name" })}
                        onMouseLeave={() => setHoveredField(null)}
                      >
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
                              {isHoveringName && (
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
                            placeholder="Add description..."
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
                            {isHoveringDescription && (
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
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon" className="h-8 w-8 hover:bg-muted cursor-pointer">
                          <IconDotsVertical className="h-4 w-4" />
                          <span className="sr-only">{t("filesTable.actions.menu")}</span>
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" className="w-[200px]">
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onPreview(file)}>
                          <IconEye className="h-4 w-4" />
                          {t("filesTable.actions.preview")}
                        </DropdownMenuItem>
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onRename(file)}>
                          <IconEdit className="h-4 w-4" />
                          {t("filesTable.actions.edit")}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={() => onDownload(file.objectName, file.name)}
                        >
                          <IconDownload className="h-4 w-4" />
                          {t("filesTable.actions.download")}
                        </DropdownMenuItem>
                        <DropdownMenuItem className="cursor-pointer py-2" onClick={() => onShare(file)}>
                          <IconShare className="h-4 w-4" />
                          {t("filesTable.actions.share")}
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          onClick={() => onDelete(file)}
                          className="cursor-pointer py-2 text-destructive focus:text-destructive"
                        >
                          <IconTrash className="h-4 w-4" />
                          {t("filesTable.actions.delete")}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
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
