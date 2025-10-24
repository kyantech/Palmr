"use client";

import { useCallback, useEffect, useState } from "react";
import { IconCloudUpload, IconLoader, IconX } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useUppyUpload } from "@/hooks/useUppyUpload";
import { checkFile, getFilePresignedUrl, registerFile } from "@/http/endpoints";
import { getFileIcon } from "@/utils/file-icons";
import { generateSafeFileName } from "@/utils/file-utils";
import { formatFileSize } from "@/utils/format-file-size";
import getErrorData from "@/utils/getErrorData";

interface GlobalDropZoneProps {
  onSuccess?: () => void;
  children: React.ReactNode;
  currentFolderId?: string;
}

export function GlobalDropZone({ onSuccess, children, currentFolderId }: GlobalDropZoneProps) {
  const t = useTranslations();
  const [isDragOver, setIsDragOver] = useState(false);

  const { addFiles, startUpload, fileUploads, removeFile, retryUpload } = useUppyUpload({
    onValidate: async (file) => {
      const fileName = file.name;
      const extension = fileName.split(".").pop() || "";
      const safeObjectName = generateSafeFileName(fileName);

      try {
        await checkFile({
          name: fileName,
          objectName: safeObjectName,
          size: file.size,
          extension: extension,
          folderId: currentFolderId,
        });
      } catch (error) {
        console.error("File check failed:", error);
        const errorData = getErrorData(error);
        let errorMessage = t("uploadFile.error");

        if (errorData.code === "fileSizeExceeded") {
          errorMessage = t(`uploadFile.${errorData.code}`, { maxsizemb: errorData.details || "0" });
        } else if (errorData.code === "insufficientStorage") {
          errorMessage = t(`uploadFile.${errorData.code}`, { availablespace: errorData.details || "0" });
        } else if (errorData.code) {
          errorMessage = t(`uploadFile.${errorData.code}`);
        }

        toast.error(errorMessage);
        throw new Error(errorMessage);
      }
    },
    onBeforeUpload: async (file) => {
      const safeObjectName = generateSafeFileName(file.name);
      return safeObjectName;
    },
    getPresignedUrl: async (objectName, extension) => {
      const response = await getFilePresignedUrl({
        filename: objectName.replace(`.${extension}`, ""),
        extension,
      });

      // IMPORTANT: Use the objectName returned by backend, not the one we generated!
      // The backend generates: userId/timestamp-random-filename.extension
      const actualObjectName = response.data.objectName;

      return { url: response.data.url, method: "PUT", actualObjectName };
    },
    onAfterUpload: async (fileId, file, objectName) => {
      const fileName = file.name;
      const extension = fileName.split(".").pop() || "";

      await registerFile({
        name: fileName,
        objectName,
        size: file.size,
        extension,
        folderId: currentFolderId,
      });
    },
  });

  // Monitor upload completion separately from the hook
  useEffect(() => {
    // Only process if we have uploads completed
    if (fileUploads.length === 0) return;

    const successCount = fileUploads.filter((u) => u.status === "success").length;
    const errorCount = fileUploads.filter((u) => u.status === "error").length;
    const pendingCount = fileUploads.filter((u) => u.status === "pending" || u.status === "uploading").length;

    // All uploads are done (no pending/uploading)
    if (pendingCount === 0 && successCount > 0) {
      toast.success(
        errorCount > 0
          ? t("uploadFile.partialSuccess", { success: successCount, error: errorCount })
          : t("uploadFile.allSuccess", { count: successCount })
      );

      onSuccess?.();

      // Auto-remove successful uploads after 3 seconds
      setTimeout(() => {
        fileUploads.forEach((upload) => {
          if (upload.status === "success") {
            removeFile(upload.id);
          }
        });
      }, 3000);
    }
  }, [fileUploads, onSuccess, removeFile, t]);

  const handleDragOver = useCallback((event: DragEvent) => {
    // Check if this is a move operation (dragging existing items)
    const isMoveOperation = event.dataTransfer?.types.includes("application/x-move-item");
    if (isMoveOperation) {
      return; // Don't interfere with move operations
    }

    event.preventDefault();
    event.stopPropagation();
    setIsDragOver(true);
  }, []);

  const handleDragLeave = useCallback((event: DragEvent) => {
    // Check if this is a move operation (dragging existing items)
    const isMoveOperation = event.dataTransfer?.types.includes("application/x-move-item");
    if (isMoveOperation) {
      return; // Don't interfere with move operations
    }

    event.preventDefault();
    event.stopPropagation();
    setIsDragOver(false);
  }, []);

  const handleDrop = useCallback(
    (event: DragEvent) => {
      // Check if this is a move operation (dragging existing items)
      const isMoveOperation = event.dataTransfer?.types.includes("application/x-move-item");
      if (isMoveOperation) {
        return; // Don't interfere with move operations
      }

      event.preventDefault();
      event.stopPropagation();
      setIsDragOver(false);

      const files = event.dataTransfer?.files;
      if (!files || files.length === 0) return;

      const filesArray = Array.from(files);
      addFiles(filesArray);
      toast.info(t("uploadFile.filesQueued", { count: filesArray.length }));
    },
    [addFiles, t]
  );

  const handlePaste = useCallback(
    (event: ClipboardEvent) => {
      const target = event.target as HTMLElement;
      const isInput = target.tagName === "INPUT" || target.tagName === "TEXTAREA";
      const isPasswordInput = target.tagName === "INPUT" && (target as HTMLInputElement).type === "password";

      if (isInput && !isPasswordInput) {
        return;
      }

      const items = event.clipboardData?.items;
      if (!items) return;

      const imageItems = Array.from(items).filter((item) => item.type.startsWith("image/"));

      if (imageItems.length === 0) return;

      event.preventDefault();
      event.stopPropagation();

      const newFiles: File[] = [];

      imageItems.forEach((item) => {
        const file = item.getAsFile();
        if (file) {
          const timestamp = Date.now();
          const extension = file.type.split("/")[1] || "png";
          const fileName = `pasted-${timestamp}.${extension}`;

          const renamedFile = new File([file], fileName, { type: file.type });
          newFiles.push(renamedFile);
        }
      });

      if (newFiles.length > 0) {
        addFiles(newFiles);
        toast.success(t("uploadFile.pasteSuccess", { count: newFiles.length }));
      }
    },
    [addFiles, t]
  );

  // Auto-start uploads when files are added
  useEffect(() => {
    if (fileUploads.some((f) => f.status === "pending")) {
      // Wait a bit for all validations, then start upload
      const timer = setTimeout(() => {
        startUpload();
      }, 200);

      return () => clearTimeout(timer);
    }
  }, [fileUploads, startUpload]);

  useEffect(() => {
    document.addEventListener("dragover", handleDragOver);
    document.addEventListener("dragleave", handleDragLeave);
    document.addEventListener("drop", handleDrop);
    document.addEventListener("paste", handlePaste);

    return () => {
      document.removeEventListener("dragover", handleDragOver);
      document.removeEventListener("dragleave", handleDragLeave);
      document.removeEventListener("drop", handleDrop);
      document.removeEventListener("paste", handlePaste);
    };
  }, [handleDragOver, handleDragLeave, handleDrop, handlePaste]);

  const renderFileIcon = (fileName: string) => {
    const { icon: FileIcon, color } = getFileIcon(fileName);
    return <FileIcon size={16} className={color} />;
  };

  const getStatusIcon = (status: string) => {
    switch (status) {
      case "uploading":
        return <IconLoader size={14} className="animate-spin text-blue-500" />;
      case "success":
        return <IconCloudUpload size={14} className="text-green-500" />;
      case "error":
        return <IconX size={14} className="text-red-500" />;
      default:
        return null;
    }
  };

  return (
    <>
      {children}

      {isDragOver && (
        <div className="fixed inset-0 z-50 dark:bg-black/80 bg-white/90 border-2 border-dashed dark:border-primary/50 border-primary/90 rounded-lg m-1 flex items-center justify-center">
          <div className="text-center">
            <IconCloudUpload size={64} className="text-primary mx-auto mb-4" />
            <h3 className="text-2xl font-bold text-primary mb-2">{t("uploadFile.globalDrop.title")}</h3>
            <p className="text-lg dark:text-muted-foreground text-black">{t("uploadFile.globalDrop.description")}</p>
          </div>
        </div>
      )}

      {fileUploads.length > 0 && (
        <div className="fixed bottom-4 right-4 z-50 max-w-sm w-full space-y-2">
          {fileUploads.map((upload) => (
            <div key={upload.id} className="bg-background border rounded-lg shadow-lg p-3 flex items-center gap-3">
              <div className="flex-shrink-0">{renderFileIcon(upload.file.name)}</div>

              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <p className="text-sm font-medium truncate">{upload.file.name}</p>
                  {getStatusIcon(upload.status)}
                </div>
                <p className="text-xs text-muted-foreground">{formatFileSize(upload.file.size)}</p>

                {upload.status === "uploading" && (
                  <div className="mt-1">
                    <Progress value={upload.progress} className="h-1" />
                    <p className="text-xs text-muted-foreground mt-1">{upload.progress}%</p>
                  </div>
                )}

                {upload.status === "error" && upload.error && (
                  <p className="text-xs text-destructive mt-1">{upload.error}</p>
                )}
              </div>

              <div className="flex-shrink-0">
                {upload.status === "error" ? (
                  <div className="flex gap-1">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => retryUpload(upload.id)}
                      className="h-6 w-6 p-0"
                      title={t("uploadFile.retry")}
                    >
                      <IconLoader size={12} />
                    </Button>
                    <Button variant="ghost" size="sm" onClick={() => removeFile(upload.id)} className="h-6 w-6 p-0">
                      <IconX size={12} />
                    </Button>
                  </div>
                ) : upload.status === "success" ? null : (
                  <Button variant="ghost" size="sm" onClick={() => removeFile(upload.id)} className="h-6 w-6 p-0">
                    <IconX size={12} />
                  </Button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}
