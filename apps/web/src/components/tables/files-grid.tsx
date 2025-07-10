import { useEffect, useRef, useState } from "react";
import {
  IconChevronDown,
  IconDotsVertical,
  IconDownload,
  IconEdit,
  IconEye,
  IconShare,
  IconTrash,
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
import { getDownloadUrl } from "@/http/endpoints";
import { getFileIcon } from "@/utils/file-icons";
import { formatFileSize } from "@/utils/format-file-size";

const urlCache: Record<string, { url: string; timestamp: number }> = {};
const CACHE_DURATION = 1000 * 60;

interface File {
  id: string;
  name: string;
  description?: string;
  size: number;
  objectName: string;
  createdAt: string;
  updatedAt: string;
}

interface FilesGridProps {
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

export function FilesGrid({
  files,
  onPreview,
  onRename,
  onDownload,
  onShare,
  onDelete,
  onBulkDelete,
  onBulkShare,
  onBulkDownload,
  setClearSelectionCallback,
}: FilesGridProps) {
  const t = useTranslations();

  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [filePreviewUrls, setFilePreviewUrls] = useState<Record<string, string>>({});

  const loadingUrls = useRef<Set<string>>(new Set());
  const componentMounted = useRef(true);

  useEffect(() => {
    componentMounted.current = true;
    return () => {
      componentMounted.current = false;
      Object.keys(urlCache).forEach((key) => delete urlCache[key]);
    };
  }, []);

  useEffect(() => {
    const clearSelection = () => setSelectedFiles(new Set());
    setClearSelectionCallback?.(clearSelection);
  }, [setClearSelectionCallback]);

  const isImageFile = (fileName: string) => {
    const imageExtensions = [".jpg", ".jpeg", ".png", ".gif", ".webp"];
    return imageExtensions.some((ext) => fileName.toLowerCase().endsWith(ext));
  };

  useEffect(() => {
    const loadPreviewUrls = async () => {
      const imageFiles = files.filter((file) => isImageFile(file.name));
      const now = Date.now();

      for (const file of imageFiles) {
        if (!componentMounted.current) break;
        if (loadingUrls.current.has(file.objectName)) {
          continue;
        }
        if (filePreviewUrls[file.id]) {
          continue;
        }

        const cached = urlCache[file.objectName];
        if (cached && now - cached.timestamp < CACHE_DURATION) {
          setFilePreviewUrls((prev) => ({ ...prev, [file.id]: cached.url }));
          continue;
        }

        try {
          loadingUrls.current.add(file.objectName);
          const encodedObjectName = encodeURIComponent(file.objectName);
          const response = await getDownloadUrl(encodedObjectName);

          if (!componentMounted.current) break;

          urlCache[file.objectName] = { url: response.data.url, timestamp: now };
          setFilePreviewUrls((prev) => ({ ...prev, [file.id]: response.data.url }));
        } catch (error) {
          console.error(`Failed to load preview for ${file.name}:`, error);
        } finally {
          loadingUrls.current.delete(file.objectName);
        }
      }
    };

    if (componentMounted.current) {
      loadPreviewUrls();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [files]);

  const formatDateTime = (dateString: string) => {
    const date = new Date(dateString);
    return new Intl.DateTimeFormat("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
    }).format(date);
  };

  const handleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedFiles(new Set(files.map((file) => file.id)));
    } else {
      setSelectedFiles(new Set());
    }
  };

  const handleSelectFile = (e: React.MouseEvent, fileId: string, checked: boolean) => {
    e.stopPropagation();
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

      <div className="flex items-center gap-2 px-2">
        <Checkbox checked={isAllSelected} onCheckedChange={handleSelectAll} aria-label={t("filesTable.selectAll")} />
        <span className="text-sm text-muted-foreground">{t("filesTable.selectAll")}</span>
      </div>

      <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-4">
        {files.map((file) => {
          const { icon: FileIcon, color } = getFileIcon(file.name);
          const isSelected = selectedFiles.has(file.id);
          const isImage = isImageFile(file.name);
          const previewUrl = filePreviewUrls[file.id];

          return (
            <div
              key={file.id}
              className={`relative group border rounded-lg p-3 hover:bg-muted/50 transition-colors cursor-pointer ${
                isSelected ? "ring-2 ring-primary bg-muted/50" : ""
              }`}
              onClick={(e) => {
                if (
                  (e.target as HTMLElement).closest(".checkbox-wrapper") ||
                  (e.target as HTMLElement).closest('[role="menuitem"]')
                ) {
                  return;
                }
                onPreview(file);
              }}
            >
              <div className="absolute top-2 left-2 z-10 checkbox-wrapper">
                <Checkbox
                  checked={isSelected}
                  onCheckedChange={(checked: boolean) => {
                    handleSelectFile({ stopPropagation: () => {} } as React.MouseEvent, file.id, checked);
                  }}
                  aria-label={t("filesTable.selectFile", { fileName: file.name })}
                  className="bg-background border-2"
                />
              </div>

              <div className="absolute top-2 right-2 z-10">
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost" size="icon" className="h-8 w-8" onClick={(e) => e.stopPropagation()}>
                      <IconDotsVertical className="h-4 w-4" />
                      <span className="sr-only">{t("filesTable.actions.menu")}</span>
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" className="w-[200px]">
                    <DropdownMenuItem
                      className="cursor-pointer py-2"
                      onClick={(e) => {
                        e.stopPropagation();
                        onPreview(file);
                      }}
                    >
                      <IconEye className="h-4 w-4" />
                      {t("filesTable.actions.preview")}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      className="cursor-pointer py-2"
                      onClick={(e) => {
                        e.stopPropagation();
                        onRename(file);
                      }}
                    >
                      <IconEdit className="h-4 w-4" />
                      {t("filesTable.actions.edit")}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      className="cursor-pointer py-2"
                      onClick={(e) => {
                        e.stopPropagation();
                        onDownload(file.objectName, file.name);
                      }}
                    >
                      <IconDownload className="h-4 w-4" />
                      {t("filesTable.actions.download")}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      className="cursor-pointer py-2"
                      onClick={(e) => {
                        e.stopPropagation();
                        onShare(file);
                      }}
                    >
                      <IconShare className="h-4 w-4" />
                      {t("filesTable.actions.share")}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      onClick={(e) => {
                        e.stopPropagation();
                        onDelete(file);
                      }}
                      className="cursor-pointer py-2 text-destructive focus:text-destructive"
                    >
                      <IconTrash className="h-4 w-4" />
                      {t("filesTable.actions.delete")}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>

              <div className="flex flex-col items-center space-y-3">
                <div className="w-16 h-16 flex items-center justify-center bg-muted/30 rounded-lg overflow-hidden">
                  {isImage && previewUrl ? (
                    <img src={previewUrl} alt={file.name} className="object-cover w-full h-full" />
                  ) : (
                    <FileIcon className={`h-10 w-10 ${color}`} />
                  )}
                </div>

                <div className="w-full space-y-1">
                  <p className="text-sm font-medium truncate text-left" title={file.name}>
                    {file.name}
                  </p>
                  {file.description && (
                    <p className="text-xs text-muted-foreground truncate text-left" title={file.description}>
                      {file.description}
                    </p>
                  )}
                  <div className="text-xs text-muted-foreground space-y-1 text-left">
                    <p>{formatFileSize(file.size)}</p>
                    <p>{formatDateTime(file.createdAt)}</p>
                  </div>
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
