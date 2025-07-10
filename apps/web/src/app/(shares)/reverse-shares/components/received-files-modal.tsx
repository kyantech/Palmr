"use client";

import { useEffect, useRef, useState } from "react";
import {
  IconCheck,
  IconChevronDown,
  IconClipboardCopy,
  IconDownload,
  IconEdit,
  IconEye,
  IconFile,
  IconTrash,
  IconX,
} from "@tabler/icons-react";
import { format } from "date-fns";
import { ptBR } from "date-fns/locale";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import {
  copyReverseShareFileToUserFiles,
  deleteReverseShareFile,
  downloadReverseShareFile,
  updateReverseShareFile,
} from "@/http/endpoints/reverse-shares";
import type { ReverseShareFile } from "@/http/endpoints/reverse-shares/types";
import { getFileIcon } from "@/utils/file-icons";
import { truncateFileName } from "@/utils/file-utils";
import { ReverseShare } from "../hooks/use-reverse-shares";
import { ReverseShareFilePreviewModal } from "./reverse-share-file-preview-modal";

interface EditingState {
  fileId: string;
  field: string;
}

interface HoverState {
  fileId: string;
  field: string;
}

const getFileNameWithoutExtension = (fileName: string) => {
  return fileName.replace(/\.[^/.]+$/, "");
};

function useFileEdit() {
  const [editingFile, setEditingFile] = useState<EditingState | null>(null);
  const [editValue, setEditValue] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editingFile && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingFile]);

  const startEdit = (fileId: string, field: string, currentValue: string) => {
    setEditingFile({ fileId, field });
    if (field === "name") {
      const nameWithoutExtension = getFileNameWithoutExtension(currentValue);
      setEditValue(nameWithoutExtension);
    } else {
      setEditValue(currentValue);
    }
  };

  const cancelEdit = () => {
    setEditingFile(null);
    setEditValue("");
  };

  return {
    editingFile,
    editValue,
    setEditValue,
    inputRef,
    startEdit,
    cancelEdit,
  };
}

const formatFileSize = (sizeString: string) => {
  const sizeInBytes = parseInt(sizeString);
  if (sizeInBytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const k = 1024;
  const i = Math.floor(Math.log(sizeInBytes) / Math.log(k));
  return `${parseFloat((sizeInBytes / Math.pow(k, i)).toFixed(1))} ${units[i]}`;
};

const formatDate = (dateString: string) => {
  try {
    return format(new Date(dateString), "dd/MM/yyyy HH:mm", { locale: ptBR });
  } catch {
    return "Data inválida";
  }
};

const getFileExtension = (fileName: string) => {
  const match = fileName.match(/\.[^/.]+$/);
  return match ? match[0] : "";
};

const getSenderDisplay = (file: ReverseShareFile, t: any) => {
  if (file.uploaderName && file.uploaderEmail) {
    return `${file.uploaderName} (${file.uploaderEmail})`;
  }
  if (file.uploaderName) return file.uploaderName;
  if (file.uploaderEmail) return file.uploaderEmail;
  return t("reverseShares.components.fileRow.anonymous");
};

const getSenderInitials = (file: ReverseShareFile) => {
  if (file.uploaderName) {
    return file.uploaderName
      .split(" ")
      .map((n) => n[0])
      .join("")
      .toUpperCase()
      .slice(0, 2);
  }
  if (file.uploaderEmail) {
    return file.uploaderEmail[0].toUpperCase();
  }
  return "?";
};

interface EditableFieldProps {
  file: ReverseShareFile;
  field: "name" | "description";
  isEditing: boolean;
  editValue: string;
  inputRef: React.RefObject<HTMLInputElement | null>;
  isHovered: boolean;
  onStartEdit: (fileId: string, field: string, currentValue: string) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
  onEditValueChange: (value: string) => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
}

function EditableField({
  file,
  field,
  isEditing,
  editValue,
  inputRef,
  isHovered,
  onStartEdit,
  onSaveEdit,
  onCancelEdit,
  onEditValueChange,
  onKeyDown,
}: EditableFieldProps) {
  const t = useTranslations();

  if (isEditing) {
    return (
      <div className="flex items-center gap-1 flex-1">
        {field === "name" ? (
          <div className="flex items-center">
            <Input
              ref={inputRef}
              value={editValue}
              onChange={(e) => onEditValueChange(e.target.value)}
              onKeyDown={onKeyDown}
              className="h-8 text-sm font-medium rounded-r-none border-r-0"
              onClick={(e) => e.stopPropagation()}
            />
            <div className="h-8 px-2 bg-muted border border-l-0 rounded-r text-sm font-medium flex items-center text-muted-foreground">
              {getFileExtension(file.name)}
            </div>
          </div>
        ) : (
          <Input
            ref={inputRef}
            value={editValue}
            onChange={(e) => onEditValueChange(e.target.value)}
            onKeyDown={onKeyDown}
            className="h-6 text-xs"
            placeholder={t("reverseShares.components.fileRow.addDescription")}
            onClick={(e) => e.stopPropagation()}
          />
        )}
        <Button
          size="icon"
          variant="ghost"
          className="h-5 w-5 text-green-600 hover:text-green-700 flex-shrink-0"
          onClick={(e) => {
            e.stopPropagation();
            onSaveEdit();
          }}
          title={t("reverseShares.components.editField.saveChanges")}
        >
          <IconCheck className="h-3 w-3" />
        </Button>
        <Button
          size="icon"
          variant="ghost"
          className="h-5 w-5 text-red-600 hover:text-red-700 flex-shrink-0"
          onClick={(e) => {
            e.stopPropagation();
            onCancelEdit();
          }}
          title={t("reverseShares.components.editField.cancelEdit")}
        >
          <IconX className="h-3 w-3" />
        </Button>
      </div>
    );
  }

  const currentValue = field === "name" ? file.name : file.description;
  const displayValue = field === "name" ? getFileNameWithoutExtension(file.name) : currentValue;

  return (
    <div className="flex items-center gap-1 flex-1 min-w-0">
      <div
        className={`${field === "name" ? "font-medium" : "text-sm text-muted-foreground"} truncate max-w-[200px]`}
        title={currentValue || ""}
      >
        {field === "name" ? (
          <>
            <span className="text-foreground">{displayValue}</span>
            <span className="text-muted-foreground">{getFileExtension(file.name)}</span>
          </>
        ) : (
          displayValue || ""
        )}
      </div>
      <div className="w-6 flex justify-center flex-shrink-0">
        <Button
          size="icon"
          variant="ghost"
          className={`h-5 w-5 text-muted-foreground hover:text-foreground hidden sm:block transition-opacity ${
            isHovered ? "opacity-100" : "opacity-0"
          }`}
          onClick={(e) => {
            e.stopPropagation();
            onStartEdit(file.id, field, currentValue || "");
          }}
          title={t("reverseShares.components.fileActions.edit")}
        >
          <IconEdit className="h-3 w-3" />
        </Button>
      </div>
    </div>
  );
}

interface FileRowProps {
  file: ReverseShareFile;
  editingFile: EditingState | null;
  editValue: string;
  inputRef: React.RefObject<HTMLInputElement | null>;
  hoveredFile: HoverState | null;
  copyingFile: string | null;
  isSelected: boolean;
  onStartEdit: (fileId: string, field: string, currentValue: string) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
  onEditValueChange: (value: string) => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
  onSetHoveredFile: (hover: HoverState | null) => void;
  onPreview: (file: ReverseShareFile) => void;
  onDownload: (file: ReverseShareFile) => void;
  onDelete: (file: ReverseShareFile) => void;
  onCopy: (file: ReverseShareFile) => void;
  onSelectFile: (fileId: string, checked: boolean) => void;
}

function FileRow({
  file,
  editingFile,
  editValue,
  inputRef,
  hoveredFile,
  copyingFile,
  isSelected,
  onStartEdit,
  onSaveEdit,
  onCancelEdit,
  onEditValueChange,
  onKeyDown,
  onSetHoveredFile,
  onPreview,
  onDownload,
  onDelete,
  onCopy,
  onSelectFile,
}: FileRowProps) {
  const t = useTranslations();
  const { icon: FileIcon, color } = getFileIcon(file.name);

  return (
    <TableRow key={file.id}>
      <TableCell>
        <Checkbox
          checked={isSelected}
          onCheckedChange={(checked: boolean) => onSelectFile(file.id, checked)}
          aria-label={t("reverseShares.modals.receivedFiles.selectFile", { fileName: file.name })}
        />
      </TableCell>
      <TableCell>
        <div className="flex items-center gap-3">
          <FileIcon className={`h-8 w-8 ${color} flex-shrink-0`} />
          <div className="min-w-0 flex-1">
            <div
              onMouseEnter={() => onSetHoveredFile({ fileId: file.id, field: "name" })}
              onMouseLeave={() => onSetHoveredFile(null)}
            >
              <EditableField
                file={file}
                field="name"
                isEditing={editingFile?.fileId === file.id && editingFile?.field === "name"}
                editValue={editValue}
                inputRef={inputRef}
                isHovered={hoveredFile?.fileId === file.id && hoveredFile?.field === "name"}
                onStartEdit={onStartEdit}
                onSaveEdit={onSaveEdit}
                onCancelEdit={onCancelEdit}
                onEditValueChange={onEditValueChange}
                onKeyDown={onKeyDown}
              />
            </div>
            {file.description && (
              <div
                className="mt-1"
                onMouseEnter={() => onSetHoveredFile({ fileId: file.id, field: "description" })}
                onMouseLeave={() => onSetHoveredFile(null)}
              >
                <EditableField
                  file={file}
                  field="description"
                  isEditing={editingFile?.fileId === file.id && editingFile?.field === "description"}
                  editValue={editValue}
                  inputRef={inputRef}
                  isHovered={hoveredFile?.fileId === file.id && hoveredFile?.field === "description"}
                  onStartEdit={onStartEdit}
                  onSaveEdit={onSaveEdit}
                  onCancelEdit={onCancelEdit}
                  onEditValueChange={onEditValueChange}
                  onKeyDown={onKeyDown}
                />
              </div>
            )}
          </div>
        </div>
      </TableCell>
      <TableCell className="font-mono text-sm">{formatFileSize(file.size)}</TableCell>
      <TableCell className="max-w-[200px]">
        <div className="flex items-center gap-2 min-w-0">
          <Avatar className="h-6 w-6 flex-shrink-0">
            <AvatarFallback className="text-xs">{getSenderInitials(file)}</AvatarFallback>
          </Avatar>
          <span className="text-sm truncate min-w-0" title={getSenderDisplay(file, t)}>
            {getSenderDisplay(file, t)}
          </span>
        </div>
      </TableCell>
      <TableCell className="text-sm text-muted-foreground">{formatDate(file.createdAt)}</TableCell>
      <TableCell className="text-right">
        <div className="flex items-center justify-end gap-1">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onPreview(file)}
            title={t("reverseShares.components.fileActions.preview")}
          >
            <IconEye className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onCopy(file)}
            disabled={copyingFile === file.id}
            title={
              copyingFile === file.id
                ? t("reverseShares.components.fileActions.copying")
                : t("reverseShares.components.fileActions.copyToMyFiles")
            }
            className="text-blue-600 hover:text-blue-700 hover:bg-blue-50 disabled:opacity-50"
          >
            {copyingFile === file.id ? (
              <div className="animate-spin rounded-full h-4 w-4 border-2 border-blue-600 border-t-transparent"></div>
            ) : (
              <IconClipboardCopy className="h-4 w-4" />
            )}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onDownload(file)}
            title={t("reverseShares.components.fileActions.download")}
          >
            <IconDownload className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onDelete(file)}
            title={t("reverseShares.components.fileActions.delete")}
            className="text-red-600 hover:text-red-700 hover:bg-red-50"
          >
            <IconTrash className="h-4 w-4" />
          </Button>
        </div>
      </TableCell>
    </TableRow>
  );
}

interface ReceivedFilesModalProps {
  reverseShare: ReverseShare | null;
  isOpen: boolean;
  onClose: () => void;
  onRefresh?: () => Promise<void>;
  refreshReverseShare?: (id: string) => Promise<void>;
}

export function ReceivedFilesModal({
  reverseShare,
  isOpen,
  onClose,
  onRefresh,
  refreshReverseShare,
}: ReceivedFilesModalProps) {
  const t = useTranslations();
  const [previewFile, setPreviewFile] = useState<ReverseShareFile | null>(null);
  const [hoveredFile, setHoveredFile] = useState<HoverState | null>(null);
  const [copyingFile, setCopyingFile] = useState<string | null>(null);
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [bulkCopying, setBulkCopying] = useState(false);
  const [bulkDeleting, setBulkDeleting] = useState(false);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [filesToDeleteBulk, setFilesToDeleteBulk] = useState<ReverseShareFile[]>([]);

  const { editingFile, editValue, setEditValue, inputRef, startEdit, cancelEdit } = useFileEdit();

  // Clear selections when files change
  useEffect(() => {
    setSelectedFiles(new Set());
  }, [reverseShare?.files]);

  const getTotalSize = () => {
    if (!reverseShare?.files) return "0 B";
    const totalBytes = reverseShare.files.reduce((acc, file) => acc + parseInt(file.size), 0);
    return formatFileSize(totalBytes.toString());
  };

  const handleDownload = async (file: ReverseShareFile) => {
    try {
      const response = await downloadReverseShareFile(file.id);
      const downloadUrl = response.data.url;

      const fileResponse = await fetch(downloadUrl);
      if (!fileResponse.ok) {
        throw new Error(`Download failed: ${fileResponse.status}`);
      }

      const blob = await fileResponse.blob();
      const url = window.URL.createObjectURL(blob);

      const link = document.createElement("a");
      link.href = url;
      link.download = file.name;
      document.body.appendChild(link);
      link.click();

      document.body.removeChild(link);
      window.URL.revokeObjectURL(url);

      toast.success(t("reverseShares.modals.receivedFiles.downloadSuccess"));
    } catch (error) {
      console.error("Download error:", error);
      toast.error(t("reverseShares.modals.receivedFiles.downloadError"));
    }
  };

  const handlePreview = (file: ReverseShareFile) => {
    setPreviewFile(file);
  };

  const saveEdit = async () => {
    if (!editingFile) return;

    try {
      const updateData: { name?: string; description?: string | null } = {};

      if (editingFile.field === "name") {
        updateData.name = editValue.trim();
      } else if (editingFile.field === "description") {
        updateData.description = editValue.trim() || null;
      }

      await updateReverseShareFile(editingFile.fileId, updateData);

      if (refreshReverseShare && reverseShare) {
        await refreshReverseShare(reverseShare.id);
      } else if (onRefresh) {
        await onRefresh();
      }

      toast.success(t("reverseShares.modals.receivedFiles.editSuccess"));
    } catch (error) {
      console.error("Error updating file:", error);
      toast.error(t("reverseShares.modals.receivedFiles.editError"));
    } finally {
      cancelEdit();
    }
  };

  const handleDeleteFile = async (file: ReverseShareFile) => {
    try {
      await deleteReverseShareFile(file.id);

      if (refreshReverseShare && reverseShare) {
        await refreshReverseShare(reverseShare.id);
      } else if (onRefresh) {
        await onRefresh();
      }

      toast.success(t("reverseShares.modals.receivedFiles.deleteSuccess"));
    } catch (error) {
      console.error("Error deleting file:", error);
      toast.error(t("reverseShares.modals.receivedFiles.deleteError"));
    }
  };

  const handleCopyFile = async (file: ReverseShareFile) => {
    try {
      setCopyingFile(file.id);
      await copyReverseShareFileToUserFiles(file.id);
      toast.success(t("reverseShares.modals.receivedFiles.copySuccess"));
    } catch (error: any) {
      console.error("Error copying file:", error);

      if (error.response?.data?.error) {
        const errorMessage = error.response.data.error;
        if (errorMessage.includes("File size exceeds") || errorMessage.includes("Insufficient storage")) {
          toast.error(errorMessage);
        } else {
          toast.error(t("reverseShares.modals.receivedFiles.copyError"));
        }
      } else {
        toast.error(t("reverseShares.modals.receivedFiles.copyError"));
      }
    } finally {
      setCopyingFile(null);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      saveEdit();
    } else if (e.key === "Escape") {
      cancelEdit();
    }
  };

  if (!reverseShare) return null;

  const files = reverseShare.files || [];

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

  const getSelectedFileObjects = () => {
    return files.filter((file) => selectedFiles.has(file.id));
  };

  const isAllSelected = files.length > 0 && selectedFiles.size === files.length;

  const handleBulkDownload = async () => {
    const selectedFileObjects = getSelectedFileObjects();
    if (selectedFileObjects.length === 0) return;

    try {
      toast.promise(
        (async () => {
          const JSZip = (await import("jszip")).default;
          const zip = new JSZip();

          const downloadPromises = selectedFileObjects.map(async (file) => {
            try {
              const response = await downloadReverseShareFile(file.id);
              const downloadUrl = response.data.url;
              const fileResponse = await fetch(downloadUrl);

              if (!fileResponse.ok) {
                throw new Error(`Failed to download ${file.name}`);
              }

              const blob = await fileResponse.blob();
              zip.file(file.name, blob);
            } catch (error) {
              console.error(`Error downloading file ${file.name}:`, error);
              throw error;
            }
          });

          await Promise.all(downloadPromises);

          const zipBlob = await zip.generateAsync({ type: "blob" });
          const zipName = `${reverseShare.name || t("reverseShares.defaultLinkName")}_files.zip`;

          const url = URL.createObjectURL(zipBlob);
          const a = document.createElement("a");
          a.href = url;
          a.download = zipName;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
          URL.revokeObjectURL(url);

          // Clear selections after successful download
          setSelectedFiles(new Set());
        })(),
        {
          loading: t("shareManager.creatingZip"),
          success: t("shareManager.zipDownloadSuccess"),
          error: t("shareManager.zipDownloadError"),
        }
      );
    } catch (error) {
      console.error("Error creating ZIP:", error);
    }
  };

  const handleBulkCopyToMyFiles = async () => {
    const selectedFileObjects = getSelectedFileObjects();
    if (selectedFileObjects.length === 0) return;

    toast.promise(
      (async () => {
        setBulkCopying(true);
        try {
          const copyPromises = selectedFileObjects.map(async (file) => {
            try {
              await copyReverseShareFileToUserFiles(file.id);
            } catch (error: any) {
              console.error(`Error copying file ${file.name}:`, error);
              throw new Error(`Failed to copy ${file.name}: ${error.response?.data?.error || error.message}`);
            }
          });

          await Promise.all(copyPromises);

          // Clear selections after successful copy
          setSelectedFiles(new Set());
        } finally {
          setBulkCopying(false);
        }
      })(),
      {
        loading: t("reverseShares.modals.receivedFiles.bulkCopyProgress", { count: selectedFileObjects.length }),
        success: t("reverseShares.modals.receivedFiles.bulkCopySuccess", { count: selectedFileObjects.length }),
        error: (error: any) => {
          if (error.message.includes("File size exceeds") || error.message.includes("Insufficient storage")) {
            return error.message;
          } else {
            return t("reverseShares.modals.receivedFiles.copyError");
          }
        },
      }
    );
  };

  const handleBulkDelete = () => {
    const selectedFileObjects = getSelectedFileObjects();
    if (selectedFileObjects.length === 0) return;

    setFilesToDeleteBulk(selectedFileObjects);
    setShowDeleteConfirm(true);
  };

  const confirmBulkDelete = async () => {
    if (filesToDeleteBulk.length === 0) return;

    setShowDeleteConfirm(false);

    toast.promise(
      (async () => {
        setBulkDeleting(true);
        try {
          const deletePromises = filesToDeleteBulk.map(async (file) => {
            try {
              await deleteReverseShareFile(file.id);
            } catch (error) {
              console.error(`Error deleting file ${file.name}:`, error);
              throw new Error(`Failed to delete ${file.name}`);
            }
          });

          await Promise.all(deletePromises);

          // Clear selections and refresh data
          setSelectedFiles(new Set());
          setFilesToDeleteBulk([]);
          if (onRefresh) {
            await onRefresh();
          }
          if (refreshReverseShare) {
            await refreshReverseShare(reverseShare.id);
          }
        } finally {
          setBulkDeleting(false);
        }
      })(),
      {
        loading: t("reverseShares.modals.receivedFiles.bulkDeleteProgress", { count: filesToDeleteBulk.length }),
        success: t("reverseShares.modals.receivedFiles.bulkDeleteSuccess", { count: filesToDeleteBulk.length }),
        error: "Error deleting selected files",
      }
    );
  };

  const showBulkActions = selectedFiles.size > 0;

  return (
    <>
      <Dialog open={isOpen} onOpenChange={onClose}>
        <DialogContent className="sm:max-w-5xl max-h-[90vh] overflow-hidden flex flex-col">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <IconFile size={20} />
              {t("reverseShares.modals.receivedFiles.title")}
            </DialogTitle>
            <DialogDescription>{t("reverseShares.modals.receivedFiles.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-4 flex-1 overflow-hidden">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-4">
                <Badge variant="secondary" className="text-sm">
                  {t("reverseShares.modals.receivedFiles.fileCount", { count: files.length })}
                </Badge>
                <Badge variant="outline" className="text-sm">
                  {t("reverseShares.modals.receivedFiles.totalSize", { size: getTotalSize() })}
                </Badge>
              </div>
            </div>

            <Separator />

            {showBulkActions && (
              <div className="flex items-center justify-between p-4 bg-muted/30 border rounded-lg">
                <div className="flex items-center gap-3">
                  <span className="text-sm font-medium text-foreground">
                    {t("reverseShares.modals.receivedFiles.bulkActions.selected", { count: selectedFiles.size })}
                  </span>
                </div>
                <div className="flex items-center gap-2">
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="default" size="sm" className="gap-2">
                        {t("reverseShares.modals.receivedFiles.bulkActions.actions")}
                        <IconChevronDown className="h-4 w-4" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="w-[200px]">
                      <DropdownMenuItem className="cursor-pointer py-2" onClick={handleBulkDownload}>
                        <IconDownload className="h-4 w-4" />
                        {t("reverseShares.modals.receivedFiles.bulkActions.download")}
                      </DropdownMenuItem>
                      <DropdownMenuItem
                        className="cursor-pointer py-2"
                        onClick={handleBulkCopyToMyFiles}
                        disabled={bulkCopying}
                      >
                        {bulkCopying ? (
                          <div className="animate-spin rounded-full h-4 w-4 border-2 border-blue-600 border-t-transparent"></div>
                        ) : (
                          <IconClipboardCopy className="h-4 w-4" />
                        )}
                        {t("reverseShares.modals.receivedFiles.bulkActions.copyToMyFiles")}
                      </DropdownMenuItem>
                      <DropdownMenuItem
                        className="cursor-pointer py-2 text-destructive focus:text-destructive"
                        onClick={handleBulkDelete}
                        disabled={bulkDeleting}
                      >
                        {bulkDeleting ? (
                          <div className="animate-spin rounded-full h-4 w-4 border-2 border-red-600 border-t-transparent"></div>
                        ) : (
                          <IconTrash className="h-4 w-4" />
                        )}
                        {t("reverseShares.modals.receivedFiles.bulkActions.delete")}
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                  <Button variant="outline" size="sm" onClick={() => setSelectedFiles(new Set())}>
                    {t("common.cancel")}
                  </Button>
                </div>
              </div>
            )}

            {files.length === 0 ? (
              <div className="flex flex-col items-center justify-center flex-1 gap-4 py-12">
                <div className="w-16 h-16 bg-muted rounded-full flex items-center justify-center">
                  <IconFile className="h-8 w-8 text-muted-foreground" />
                </div>
                <div className="text-center space-y-2">
                  <h3 className="text-lg font-medium">{t("reverseShares.modals.receivedFiles.noFiles")}</h3>
                  <p className="text-muted-foreground max-w-md">
                    {t("reverseShares.modals.receivedFiles.noFilesDescription")}
                  </p>
                </div>
              </div>
            ) : (
              <ScrollArea className="flex-1">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className="w-[50px]">
                        <Checkbox
                          checked={isAllSelected}
                          onCheckedChange={handleSelectAll}
                          aria-label={t("reverseShares.modals.receivedFiles.selectAll")}
                        />
                      </TableHead>
                      <TableHead>{t("reverseShares.modals.receivedFiles.columns.file")}</TableHead>
                      <TableHead>{t("reverseShares.modals.receivedFiles.columns.size")}</TableHead>
                      <TableHead>{t("reverseShares.modals.receivedFiles.columns.sender")}</TableHead>
                      <TableHead>{t("reverseShares.modals.receivedFiles.columns.date")}</TableHead>
                      <TableHead className="text-right">
                        {t("reverseShares.modals.receivedFiles.columns.actions")}
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {files.map((file) => (
                      <FileRow
                        key={file.id}
                        file={file}
                        editingFile={editingFile}
                        editValue={editValue}
                        inputRef={inputRef}
                        hoveredFile={hoveredFile}
                        copyingFile={copyingFile}
                        isSelected={selectedFiles.has(file.id)}
                        onStartEdit={startEdit}
                        onSaveEdit={saveEdit}
                        onCancelEdit={cancelEdit}
                        onEditValueChange={setEditValue}
                        onKeyDown={handleKeyDown}
                        onSetHoveredFile={setHoveredFile}
                        onPreview={handlePreview}
                        onDownload={handleDownload}
                        onDelete={handleDeleteFile}
                        onCopy={handleCopyFile}
                        onSelectFile={handleSelectFile}
                      />
                    ))}
                  </TableBody>
                </Table>
              </ScrollArea>
            )}
          </div>
        </DialogContent>
      </Dialog>

      {/* Delete Confirmation Modal */}
      <Dialog open={showDeleteConfirm} onOpenChange={setShowDeleteConfirm}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("reverseShares.modals.receivedFiles.bulkDeleteConfirmTitle")}</DialogTitle>
            <DialogDescription>
              {t("reverseShares.modals.receivedFiles.bulkDeleteConfirmMessage", { count: filesToDeleteBulk.length })}
            </DialogDescription>
          </DialogHeader>

          <div className="max-h-48 overflow-y-auto border rounded-lg p-2">
            <div className="space-y-1">
              {filesToDeleteBulk.map((file) => {
                const { icon: FileIcon, color } = getFileIcon(file.name);
                const displayName = truncateFileName(file.name);
                return (
                  <div key={file.id} className="flex items-center gap-2 p-2 bg-muted/20 rounded text-sm min-w-0">
                    <FileIcon className={`h-4 w-4 ${color} flex-shrink-0`} />
                    <span className="flex-1 break-all" title={file.name}>
                      {displayName}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>

          <DialogFooter className="flex gap-2">
            <Button variant="outline" onClick={() => setShowDeleteConfirm(false)}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" onClick={confirmBulkDelete} disabled={bulkDeleting}>
              {bulkDeleting ? (
                <div className="animate-spin rounded-full h-4 w-4 border-2 border-white border-t-transparent mr-2" />
              ) : null}
              {t("reverseShares.modals.receivedFiles.bulkDeleteConfirmButton", { count: filesToDeleteBulk.length })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {previewFile && (
        <ReverseShareFilePreviewModal
          isOpen={!!previewFile}
          onClose={() => setPreviewFile(null)}
          file={{
            id: previewFile.id,
            name: previewFile.name,
            objectName: previewFile.objectName,
            extension: previewFile.extension,
          }}
        />
      )}
    </>
  );
}
