"use client";

import { useState } from "react";
import { IconDownload, IconEye, IconTrash } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { deleteReverseShareFile } from "@/http/endpoints/reverse-shares";
import type { ReverseShareFile } from "@/http/endpoints/reverse-shares/types";
import { getCachedDownloadUrl } from "@/lib/download-url-cache";
import { getFileIcon } from "@/utils/file-icons";
import { ReverseShareFilePreviewModal } from "./reverse-share-file-preview-modal";

interface ReceivedFilesSectionProps {
  files: ReverseShareFile[];
  onFileDeleted?: () => void;
}

export function ReceivedFilesSection({ files, onFileDeleted }: ReceivedFilesSectionProps) {
  const t = useTranslations();
  const [previewFile, setPreviewFile] = useState<ReverseShareFile | null>(null);

  const getSenderDisplay = (file: ReverseShareFile) => {
    if (file.uploaderName && file.uploaderEmail) {
      return `${file.uploaderName}(${file.uploaderEmail})`;
    }
    if (file.uploaderName) return file.uploaderName;
    if (file.uploaderEmail) return file.uploaderEmail;
    return t("reverseShares.components.fileRow.anonymous");
  };

  const formatFileSize = (size: string | number | null) => {
    if (!size) return "0 B";
    const sizeInBytes = typeof size === "string" ? parseInt(size) : size;
    if (sizeInBytes === 0) return "0 B";
    const units = ["B", "KB", "MB", "GB"];
    const k = 1024;
    const i = Math.floor(Math.log(sizeInBytes) / Math.log(k));
    return `${parseFloat((sizeInBytes / Math.pow(k, i)).toFixed(1))} ${units[i]}`;
  };

  const formatDate = (dateString: string) => {
    try {
      return new Date(dateString).toLocaleDateString("pt-BR", {
        day: "2-digit",
        month: "2-digit",
        year: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return t("reverseShares.modals.details.invalidDate");
    }
  };

  const handleDownload = async (file: ReverseShareFile) => {
    try {
      const loadingToast = toast.loading(t("reverseShares.modals.details.downloading") || "Downloading...");
      // Use same-origin proxy URL to avoid Safari cross-site tracking issues
      const url = await getCachedDownloadUrl(file.objectName);

      const link = document.createElement("a");
      link.href = url;
      link.download = file.name;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);

      toast.dismiss(loadingToast);
      toast.success(t("reverseShares.modals.details.downloadSuccess"));
    } catch (error) {
      console.error("Download error:", error);
      toast.error(t("reverseShares.modals.details.downloadError"));
    }
  };

  const handleDeleteFile = async (file: ReverseShareFile) => {
    try {
      await deleteReverseShareFile(file.id);
      toast.success(t("fileManager.deleteSuccess"));
      onFileDeleted?.();
    } catch (error) {
      console.error("Error deleting file:", error);
      toast.error(t("fileManager.deleteError"));
    }
  };

  if (!files.length) {
    return null;
  }

  return (
    <>
      <div className="space-y-3">
        <h3 className="text-base font-medium text-foreground border-b pb-2">
          {t("reverseShares.modals.details.files")} ({files.length})
        </h3>
        <div className="border rounded-lg bg-muted/10 p-2">
          <div className="grid gap-1 max-h-40 overflow-y-auto">
            {files.map((file) => {
              const { icon: FileIcon, color } = getFileIcon(file.name);
              return (
                <div key={file.id} className="flex items-center gap-2 p-2 bg-background rounded border mr-2 group">
                  <FileIcon className={`h-3.5 w-3.5 ${color} flex-shrink-0`} />
                  <div className="flex-1 min-w-0">
                    <div className="text-xs font-medium truncate max-w-[200px]" title={file.name}>
                      {file.name}
                    </div>
                    <div className="flex items-center gap-2 text-xs text-muted-foreground">
                      <span>{formatFileSize(file.size)}</span>
                      {(file.uploaderName || file.uploaderEmail) && (
                        <>
                          <span>•</span>
                          <span title={getSenderDisplay(file)}>{getSenderDisplay(file)}</span>
                        </>
                      )}
                      <span>•</span>
                      <span>{formatDate(file.createdAt)}</span>
                    </div>
                  </div>
                  <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-6 w-6 p-0"
                      onClick={() => setPreviewFile(file)}
                      title={t("reverseShares.actions.viewDetails")}
                    >
                      <IconEye className="h-3 w-3" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-6 w-6 p-0"
                      onClick={() => handleDownload(file)}
                      title={t("common.download")}
                    >
                      <IconDownload className="h-3 w-3" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-6 w-6 p-0 text-red-600 hover:text-red-700 hover:bg-red-50"
                      onClick={() => handleDeleteFile(file)}
                      title={t("common.delete")}
                    >
                      <IconTrash className="h-3 w-3" />
                    </Button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      </div>

      {previewFile && (
        <ReverseShareFilePreviewModal isOpen={!!previewFile} onClose={() => setPreviewFile(null)} file={previewFile} />
      )}
    </>
  );
}
