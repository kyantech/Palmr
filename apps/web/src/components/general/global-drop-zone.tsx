"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { IconCloudUpload, IconLoader, IconX } from "@tabler/icons-react";
import axios from "axios";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { checkFile, getFilePresignedUrl, registerFile } from "@/http/endpoints";
import { getSystemInfo } from "@/http/endpoints/app";
import { ChunkedUploader } from "@/utils/chunked-upload";
import { getFileIcon } from "@/utils/file-icons";
import { generateSafeFileName, generateUniqueFileName } from "@/utils/file-utils";
import { formatFileSize } from "@/utils/format-file-size";
import getErrorData from "@/utils/getErrorData";

interface FileItem {
  name: string;
  folderId?: string | null;
}

interface GlobalDropZoneProps {
  onSuccess?: () => void;
  children: React.ReactNode;
  currentFolderId?: string;
  existingFiles?: FileItem[];
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

export function GlobalDropZone({ onSuccess, children, currentFolderId, existingFiles = [] }: GlobalDropZoneProps) {
  const t = useTranslations();
  const [isDragOver, setIsDragOver] = useState(false);
  const [fileUploads, setFileUploads] = useState<FileUpload[]>([]);
  const [hasShownSuccessToast, setHasShownSuccessToast] = useState(false);
  const [isS3Enabled, setIsS3Enabled] = useState<boolean | null>(null);

  // Track filenames being uploaded to avoid duplicates in the same batch
  const usedFilenamesRef = useRef<Set<string>>(new Set());

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

  const calculateUploadTimeout = useCallback((fileSize: number): number => {
    const baseTimeout = 300000;
    const fileSizeMB = fileSize / (1024 * 1024);
    if (fileSizeMB > 500) {
      const extraMB = fileSizeMB - 500;
      const extraMinutes = Math.ceil(extraMB / 100);
      return baseTimeout + extraMinutes * 60000;
    }

    return baseTimeout;
  }, []);

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

        // Generate unique filename if duplicate exists
        // Also check against currently uploading files to avoid duplicates in the same batch
        const combinedFiles = [
          ...existingFiles,
          ...Array.from(usedFilenamesRef.current).map((name) => ({ name, folderId: currentFolderId })),
        ];
        const uniqueFileName = generateUniqueFileName(fileName, combinedFiles, currentFolderId);

        // Track this filename as being used
        usedFilenamesRef.current.add(uniqueFileName);

        const extension = uniqueFileName.split(".").pop() || "";
        const safeObjectName = generateSafeFileName(uniqueFileName);

        try {
          await checkFile({
            name: uniqueFileName,
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

          setFileUploads((prev) =>
            prev.map((u) => (u.id === id ? { ...u, status: UploadStatus.ERROR, error: errorMessage } : u))
          );
          return;
        }

        setFileUploads((prev) =>
          prev.map((u) => (u.id === id ? { ...u, status: UploadStatus.UPLOADING, progress: 0 } : u))
        );

        const presignedResponse = await getFilePresignedUrl({
          filename: safeObjectName.replace(`.${extension}`, ""),
          extension: extension,
        });

        const { url, objectName } = presignedResponse.data;

        setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, objectName } : u)));

        const abortController = new AbortController();
        setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, abortController } : u)));

        const shouldUseChunked = ChunkedUploader.shouldUseChunkedUpload(file.size, isS3Enabled ?? undefined);

        if (shouldUseChunked) {
          const chunkSize = ChunkedUploader.calculateOptimalChunkSize(file.size);

          const result = await ChunkedUploader.uploadFile({
            file,
            url,
            chunkSize,
            signal: abortController.signal,
            isS3Enabled: isS3Enabled ?? undefined,
            onProgress: (progress) => {
              setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, progress } : u)));
            },
          });

          if (!result.success) {
            throw new Error(result.error || "Chunked upload failed");
          }

          const finalObjectName = result.finalObjectName || objectName;

          await registerFile({
            name: uniqueFileName,
            objectName: finalObjectName,
            size: file.size,
            extension: extension,
            folderId: currentFolderId,
          });
        } else {
          const uploadTimeout = calculateUploadTimeout(file.size);

          await axios.put(url, file, {
            headers: {
              "Content-Type": file.type,
            },
            signal: abortController.signal,
            timeout: uploadTimeout, // Dynamic timeout based on file size
            maxContentLength: Infinity,
            maxBodyLength: Infinity,
            onUploadProgress: (progressEvent: any) => {
              const progress = (progressEvent.loaded / (progressEvent.total || file.size)) * 100;
              setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, progress: Math.round(progress) } : u)));
            },
          });

          await registerFile({
            name: uniqueFileName,
            objectName: objectName,
            size: file.size,
            extension: extension,
            folderId: currentFolderId,
          });
        }

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
    [t, isS3Enabled, currentFolderId, calculateUploadTimeout, existingFiles]
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
    const fetchSystemInfo = async () => {
      try {
        const response = await getSystemInfo();
        setIsS3Enabled(response.data.s3Enabled);
      } catch (error) {
        console.warn("Failed to fetch system info, defaulting to filesystem mode:", error);
        setIsS3Enabled(false);
      }
    };

    fetchSystemInfo();
  }, []);

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
          usedFilenamesRef.current.clear();
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
