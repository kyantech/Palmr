import { useEffect, useRef, useState } from "react";
import {
  IconArrowsMove,
  IconChevronDown,
  IconDotsVertical,
  IconDownload,
  IconEdit,
  IconEye,
  IconFolder,
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

interface FilesGridProps {
  files: File[];
  folders?: Folder[];
  onPreview?: (file: File) => void;
  onRename?: (file: File) => void;

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
  showBulkActions?: boolean;
  isShareMode?: boolean;
}

export function FilesGrid({
  files,
  folders = [],
  onPreview,
  onRename,
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
  showBulkActions = true,
  isShareMode = false,
}: FilesGridProps) {
  const t = useTranslations();

  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [selectedFolders, setSelectedFolders] = useState<Set<string>>(new Set());

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
    const clearSelection = () => {
      setSelectedFiles(new Set());
      setSelectedFolders(new Set());
    };
    setClearSelectionCallback?.(clearSelection);
  }, [setClearSelectionCallback]);

  const folderIds = folders?.map((f) => f.id).join(",");

  useEffect(() => {
    setSelectedFolders(new Set());
  }, [folderIds]);

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
          const response = await getDownloadUrl(file.objectName);

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
      setSelectedFolders(new Set(folders.map((folder) => folder.id)));
    } else {
      setSelectedFiles(new Set());
      setSelectedFolders(new Set());
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
              {t("filesTable.bulkActions.selected", { count: selectedItems })}
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

      <div className="flex items-center gap-2 px-2">
        <Checkbox checked={isAllSelected} onCheckedChange={handleSelectAll} aria-label={t("filesTable.selectAll")} />
        <span className="text-sm text-muted-foreground">{t("filesTable.selectAll")}</span>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-4">
        {/* Render folders first */}
        {folders.map((folder) => {
          const isSelected = selectedFolders.has(folder.id);

          return (
            <div
              key={`folder-${folder.id}`}
              className={`relative group border rounded-lg p-3 hover:bg-muted/50 transition-colors cursor-pointer ${
                isSelected ? "ring-2 ring-primary bg-muted/50" : ""
              }`}
              onClick={() => onNavigateToFolder?.(folder.id)}
            >
              <div className="absolute top-2 left-2 z-10 checkbox-wrapper">
                <Checkbox
                  checked={isSelected}
                  onCheckedChange={(checked: boolean) => {
                    const newSelected = new Set(selectedFolders);
                    if (checked) {
                      newSelected.add(folder.id);
                    } else {
                      newSelected.delete(folder.id);
                    }
                    setSelectedFolders(newSelected);
                  }}
                  aria-label={`Select folder ${folder.name}`}
                  className="bg-background border-2"
                  onClick={(e) => e.stopPropagation()}
                />
              </div>

              <div className="absolute top-2 right-2 z-10">
                {isShareMode ? (
                  onDownloadFolder && (
                    <Button
                      size="icon"
                      variant="ghost"
                      className="h-8 w-8 hover:bg-background/80"
                      onClick={(e) => {
                        e.stopPropagation();
                        onDownloadFolder(folder.id, folder.name);
                      }}
                    >
                      <IconDownload className="h-4 w-4" />
                      <span className="sr-only">{t("filesTable.actions.download")}</span>
                    </Button>
                  )
                ) : (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="ghost" size="icon" className="h-8 w-8" onClick={(e) => e.stopPropagation()}>
                        <IconDotsVertical className="h-4 w-4" />
                        <span className="sr-only">{t("filesTable.actions.menu")}</span>
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="w-[200px]">
                      {onRenameFolder && (
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={(e) => {
                            e.stopPropagation();
                            onRenameFolder(folder);
                          }}
                        >
                          <IconEdit className="h-4 w-4" />
                          {t("filesTable.actions.edit")}
                        </DropdownMenuItem>
                      )}
                      {onMoveFolder && (
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={(e) => {
                            e.stopPropagation();
                            onMoveFolder(folder);
                          }}
                        >
                          <IconArrowsMove className="h-4 w-4" />
                          {t("common.move")}
                        </DropdownMenuItem>
                      )}
                      {onShareFolder && (
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={(e) => {
                            e.stopPropagation();
                            onShareFolder(folder);
                          }}
                        >
                          <IconShare className="h-4 w-4" />
                          {t("filesTable.actions.share")}
                        </DropdownMenuItem>
                      )}
                      {onDownloadFolder && (
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={(e) => {
                            e.stopPropagation();
                            onDownloadFolder(folder.id, folder.name);
                          }}
                        >
                          <IconDownload className="h-4 w-4" />
                          {t("filesTable.actions.download")}
                        </DropdownMenuItem>
                      )}
                      {onDeleteFolder && (
                        <DropdownMenuItem
                          onClick={(e) => {
                            e.stopPropagation();
                            onDeleteFolder(folder);
                          }}
                          className="cursor-pointer py-2 text-destructive focus:text-destructive"
                        >
                          <IconTrash className="h-4 w-4" />
                          {t("filesTable.actions.delete")}
                        </DropdownMenuItem>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                )}
              </div>

              <div className="flex flex-col items-center space-y-3">
                <div className="w-16 h-16 flex items-center justify-center bg-muted/30 rounded-lg overflow-hidden">
                  <IconFolder className="h-10 w-10 text-primary" />
                </div>

                <div className="w-full space-y-1">
                  <p className="text-sm font-medium truncate text-left" title={folder.name}>
                    {folder.name}
                  </p>
                  {folder.description && (
                    <p className="text-xs text-muted-foreground truncate text-left" title={folder.description}>
                      {folder.description}
                    </p>
                  )}
                  <div className="text-xs text-muted-foreground space-y-1 text-left">
                    <p>{folder.totalSize ? formatFileSize(Number(folder.totalSize)) : "—"}</p>
                    <p>{formatDateTime(folder.createdAt)}</p>
                  </div>
                </div>
              </div>
            </div>
          );
        })}

        {/* Render files */}
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
                {isShareMode ? (
                  <Button
                    size="icon"
                    variant="ghost"
                    className="h-8 w-8 hover:bg-background/80"
                    onClick={(e) => {
                      e.stopPropagation();
                      onDownload(file.objectName, file.name);
                    }}
                  >
                    <IconDownload className="h-4 w-4" />
                    <span className="sr-only">{t("filesTable.actions.download")}</span>
                  </Button>
                ) : (
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
                          onPreview?.(file);
                        }}
                      >
                        <IconEye className="h-4 w-4" />
                        {t("filesTable.actions.preview")}
                      </DropdownMenuItem>
                      {onRename && (
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={(e) => {
                            e.stopPropagation();
                            onRename?.(file);
                          }}
                        >
                          <IconEdit className="h-4 w-4" />
                          {t("filesTable.actions.edit")}
                        </DropdownMenuItem>
                      )}
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
                      {onShare && (
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={(e) => {
                            e.stopPropagation();
                            onShare?.(file);
                          }}
                        >
                          <IconShare className="h-4 w-4" />
                          {t("filesTable.actions.share")}
                        </DropdownMenuItem>
                      )}
                      {onMoveFile && (
                        <DropdownMenuItem
                          className="cursor-pointer py-2"
                          onClick={(e) => {
                            e.stopPropagation();
                            onMoveFile?.(file);
                          }}
                        >
                          <IconArrowsMove className="h-4 w-4" />
                          {t("common.move")}
                        </DropdownMenuItem>
                      )}
                      {onDelete && (
                        <DropdownMenuItem
                          onClick={(e) => {
                            e.stopPropagation();
                            onDelete?.(file);
                          }}
                          className="cursor-pointer py-2 text-destructive focus:text-destructive"
                        >
                          <IconTrash className="h-4 w-4" />
                          {t("filesTable.actions.delete")}
                        </DropdownMenuItem>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                )}
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
