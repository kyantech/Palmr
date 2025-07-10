"use client";

import { useCallback, useEffect, useState } from "react";
import { IconCloudUpload, IconLoader, IconX } from "@tabler/icons-react";
import axios from "axios";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { checkFile, getPresignedUrl, registerFile } from "@/http/endpoints";
import { getFileIcon } from "@/utils/file-icons";
import { generateSafeFileName } from "@/utils/file-utils";
import { formatFileSize } from "@/utils/format-file-size";
import getErrorData from "@/utils/getErrorData";

interface GlobalDropZoneProps {
  onSuccess?: () => void;
  children: React.ReactNode;
}

enum UploadStatus {
  PENDING = "pending",
  UPLOADING = "uploading",
  SUCCESS = "success",
  ERROR = "error",
  CANCELLED = "cancelled",
}

interface FileUpload {
  id: string;
  file: File;
  status: UploadStatus;
  progress: number;
  error?: string;
  abortController?: AbortController;
  objectName?: string;
}

export function GlobalDropZone({ onSuccess, children }: GlobalDropZoneProps) {
  const t = useTranslations();
  const [isDragOver, setIsDragOver] = useState(false);
  const [fileUploads, setFileUploads] = useState<FileUpload[]>([]);
  const [hasShownSuccessToast, setHasShownSuccessToast] = useState(false);

  const generateFileId = useCallback(() => {
    return Date.now().toString() + Math.random().toString(36).substr(2, 9);
  }, []);

  const createFileUpload = useCallback(
    (file: File): FileUpload => {
      const id = generateFileId();
      return {
        id,
        file,
        status: UploadStatus.PENDING,
        progress: 0,
      };
    },
    [generateFileId]
  );

  const handleDragOver = useCallback((event: DragEvent) => {
    event.preventDefault();
    event.stopPropagation();
    setIsDragOver(true);
  }, []);

  const handleDragLeave = useCallback((event: DragEvent) => {
    event.preventDefault();
    event.stopPropagation();
    setIsDragOver(false);
  }, []);

  const uploadFile = useCallback(
    async (fileUpload: FileUpload) => {
      const { file, id } = fileUpload;

      try {
        const fileName = file.name;
        const extension = fileName.split(".").pop() || "";
        const safeObjectName = generateSafeFileName(fileName);

        try {
          await checkFile({
            name: fileName,
            objectName: "checkFile",
            size: file.size,
            extension: extension,
          });
        } catch (error) {
          console.error("File check failed:", error);
          const errorData = getErrorData(error);
          let errorMessage = t("uploadFile.error");

          if (errorData.code === "fileSizeExceeded") {
            errorMessage = t(`uploadFile.${errorData.code}`, { maxsizemb: t(`${errorData.details}`) });
          } else if (errorData.code === "insufficientStorage") {
            errorMessage = t(`uploadFile.${errorData.code}`, { availablespace: t(`${errorData.details}`) });
          } else if (errorData.code) {
            errorMessage = t(`uploadFile.${errorData.code}`);
          }

          setFileUploads((prev) =>
            prev.map((u) => (u.id === id ? { ...u, status: UploadStatus.ERROR, error: errorMessage } : u))
          );
          return;
        }

        setFileUploads((prev) =>
          prev.map((u) => (u.id === id ? { ...u, status: UploadStatus.UPLOADING, progress: 0 } : u))
        );

        const presignedResponse = await getPresignedUrl({
          filename: safeObjectName.replace(`.${extension}`, ""),
          extension: extension,
        });

        const { url, objectName } = presignedResponse.data;

        setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, objectName } : u)));

        const abortController = new AbortController();
        setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, abortController } : u)));

        await axios.put(url, file, {
          headers: {
            "Content-Type": file.type,
          },
          signal: abortController.signal,
          onUploadProgress: (progressEvent: any) => {
            const progress = (progressEvent.loaded / (progressEvent.total || file.size)) * 100;
            setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, progress: Math.round(progress) } : u)));
          },
        });

        await registerFile({
          name: fileName,
          objectName: objectName,
          size: file.size,
          extension: extension,
        });

        setFileUploads((prev) =>
          prev.map((u) =>
            u.id === id ? { ...u, status: UploadStatus.SUCCESS, progress: 100, abortController: undefined } : u
          )
        );
      } catch (error: any) {
        if (error.name === "AbortError" || error.code === "ERR_CANCELED") {
          return;
        }

        console.error("Upload failed:", error);
        const errorData = getErrorData(error);
        let errorMessage = t("uploadFile.error");

        if (errorData.code && errorData.code !== "error") {
          errorMessage = t(`uploadFile.${errorData.code}`);
        }

        setFileUploads((prev) =>
          prev.map((u) =>
            u.id === id ? { ...u, status: UploadStatus.ERROR, error: errorMessage, abortController: undefined } : u
          )
        );
      }
    },
    [t]
  );

  const handleDrop = useCallback(
    (event: DragEvent) => {
      event.preventDefault();
      event.stopPropagation();
      setIsDragOver(false);

      const files = event.dataTransfer?.files;
      if (!files || files.length === 0) return;

      const newUploads = Array.from(files).map(createFileUpload);
      setFileUploads((prev) => [...prev, ...newUploads]);
      setHasShownSuccessToast(false);

      newUploads.forEach((upload) => uploadFile(upload));
    },
    [uploadFile, createFileUpload]
  );

  const handlePaste = useCallback(
    (event: ClipboardEvent) => {
      event.preventDefault();
      event.stopPropagation();

      const items = event.clipboardData?.items;
      if (!items) return;

      const imageItems = Array.from(items).filter((item) => item.type.startsWith("image/"));

      if (imageItems.length === 0) return;

      const newUploads: FileUpload[] = [];

      imageItems.forEach((item) => {
        const file = item.getAsFile();
        if (file) {
          const timestamp = Date.now();
          const extension = file.type.split("/")[1] || "png";
          const fileName = `${timestamp}.${extension}`;

          const renamedFile = new File([file], fileName, { type: file.type });

          newUploads.push(createFileUpload(renamedFile));
        }
      });

      if (newUploads.length > 0) {
        setFileUploads((prev) => [...prev, ...newUploads]);
        setHasShownSuccessToast(false);

        newUploads.forEach((upload) => uploadFile(upload));

        toast.success(t("uploadFile.pasteSuccess", { count: newUploads.length }));
      }
    },
    [uploadFile, t, createFileUpload]
  );

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

  const removeFile = (fileId: string) => {
    setFileUploads((prev) => {
      const upload = prev.find((u) => u.id === fileId);
      if (upload?.abortController) {
        upload.abortController.abort();
      }
      return prev.filter((u) => u.id !== fileId);
    });
  };

  const retryUpload = (fileId: string) => {
    const upload = fileUploads.find((u) => u.id === fileId);
    if (upload) {
      setFileUploads((prev) =>
        prev.map((u) => (u.id === fileId ? { ...u, status: UploadStatus.PENDING, error: undefined, progress: 0 } : u))
      );
      uploadFile({ ...upload, status: UploadStatus.PENDING, error: undefined, progress: 0 });
    }
  };

  const renderFileIcon = (fileName: string) => {
    const { icon: FileIcon, color } = getFileIcon(fileName);
    return <FileIcon size={16} className={color} />;
  };

  const getStatusIcon = (status: UploadStatus) => {
    switch (status) {
      case UploadStatus.UPLOADING:
        return <IconLoader size={14} className="animate-spin text-blue-500" />;
      case UploadStatus.SUCCESS:
        return <IconCloudUpload size={14} className="text-green-500" />;
      case UploadStatus.ERROR:
        return <IconX size={14} className="text-red-500" />;
      default:
        return null;
    }
  };

  useEffect(() => {
    if (fileUploads.length > 0) {
      const allComplete = fileUploads.every(
        (u) => u.status === UploadStatus.SUCCESS || u.status === UploadStatus.ERROR
      );

      if (allComplete && !hasShownSuccessToast) {
        const successCount = fileUploads.filter((u) => u.status === UploadStatus.SUCCESS).length;
        const errorCount = fileUploads.filter((u) => u.status === UploadStatus.ERROR).length;

        if (successCount > 0) {
          toast.success(
            errorCount > 0
              ? t("uploadFile.partialSuccess", { success: successCount, error: errorCount })
              : t("uploadFile.allSuccess", { count: successCount })
          );
          setHasShownSuccessToast(true);
          onSuccess?.();
        }

        setTimeout(() => {
          setFileUploads([]);
          setHasShownSuccessToast(false);
        }, 3000);
      }
    }
  }, [fileUploads, hasShownSuccessToast, onSuccess, t]);

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

                {upload.status === UploadStatus.UPLOADING && (
                  <div className="mt-1">
                    <Progress value={upload.progress} className="h-1" />
                    <p className="text-xs text-muted-foreground mt-1">{upload.progress}%</p>
                  </div>
                )}

                {upload.status === UploadStatus.ERROR && upload.error && (
                  <p className="text-xs text-destructive mt-1">{upload.error}</p>
                )}
              </div>

              <div className="flex-shrink-0">
                {upload.status === UploadStatus.ERROR ? (
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
                ) : upload.status === UploadStatus.SUCCESS ? null : (
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
