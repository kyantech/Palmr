"use client";

import { useEffect, useRef, useState } from "react";
import { IconAlertTriangle, IconCheck, IconCloudUpload, IconLoader, IconTrash, IconX } from "@tabler/icons-react";
import axios from "axios";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import { checkFile, getFilePresignedUrl, registerFile } from "@/http/endpoints";
import { getSystemInfo } from "@/http/endpoints/app";
import { ChunkedUploader } from "@/utils/chunked-upload";
import { getFileIcon } from "@/utils/file-icons";
import { generateSafeFileName } from "@/utils/file-utils";
import { formatFileSize } from "@/utils/format-file-size";
import getErrorData from "@/utils/getErrorData";

interface UploadFileModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSuccess?: () => void;
  currentFolderId?: string;
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
  previewUrl?: string;
}

interface ConfirmationModalProps {
  isOpen: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  uploadsInProgress: number;
}

function ConfirmationModal({ isOpen, onConfirm, onCancel, uploadsInProgress }: ConfirmationModalProps) {
  const t = useTranslations();

  return (
    <Dialog open={isOpen} onOpenChange={() => {}}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <IconAlertTriangle size={20} className="text-amber-500" />
            {t("uploadFile.confirmCancel.title")}
          </DialogTitle>
        </DialogHeader>
        <div className="py-4">
          <p className="text-sm text-muted-foreground mb-2">
            {uploadsInProgress > 1
              ? t("uploadFile.confirmCancel.messageMultiple", { count: uploadsInProgress })
              : t("uploadFile.confirmCancel.messageSingle")}
          </p>
          <p className="text-sm text-amber-600 dark:text-amber-400">{t("uploadFile.confirmCancel.warning")}</p>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onCancel}>
            {t("uploadFile.confirmCancel.continue")}
          </Button>
          <Button variant="destructive" onClick={onConfirm}>
            {t("uploadFile.confirmCancel.cancel")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function UploadFileModal({ isOpen, onClose, onSuccess, currentFolderId }: UploadFileModalProps) {
  const t = useTranslations();
  const [fileUploads, setFileUploads] = useState<FileUpload[]>([]);
  const [isDragOver, setIsDragOver] = useState(false);
  const [showConfirmation, setShowConfirmation] = useState(false);
  const [hasShownSuccessToast, setHasShownSuccessToast] = useState(false);
  const [isS3Enabled, setIsS3Enabled] = useState<boolean | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

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
    return () => {
      fileUploads.forEach((upload) => {
        if (upload.previewUrl) {
          URL.revokeObjectURL(upload.previewUrl);
        }
      });
    };
  }, [fileUploads]);

  const generateFileId = () => {
    return Date.now().toString() + Math.random().toString(36).substr(2, 9);
  };

  const createFileUpload = (file: File): FileUpload => {
    const id = generateFileId();
    let previewUrl: string | undefined;

    if (file.type.startsWith("image/")) {
      try {
        previewUrl = URL.createObjectURL(file);
      } catch (error) {
        console.warn("Failed to create preview URL:", error);
      }
    }

    return {
      id,
      file,
      status: UploadStatus.PENDING,
      progress: 0,
      previewUrl,
    };
  };

  const handleFilesSelect = (files: FileList | null) => {
    if (!files) return;

    const newUploads = Array.from(files).map(createFileUpload);
    setFileUploads((prev) => [...prev, ...newUploads]);
    setHasShownSuccessToast(false);

    if (newUploads.length > 0) {
      toast.info(t("uploadFile.filesQueued", { count: newUploads.length }));
    }
  };

  const handleFileInputChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    handleFilesSelect(event.target.files);
    if (event.target) {
      event.target.value = "";
    }
  };

  const handleDragOver = (event: React.DragEvent) => {
    event.preventDefault();
    setIsDragOver(true);
  };

  const handleDragLeave = (event: React.DragEvent) => {
    event.preventDefault();
    setIsDragOver(false);
  };

  const handleDrop = (event: React.DragEvent) => {
    event.preventDefault();
    setIsDragOver(false);
    event.stopPropagation();

    const files = event.dataTransfer.files;
    if (files.length > 0) {
      handleFilesSelect(files);
    }
  };

  const renderFileIcon = (fileName: string) => {
    const { icon: FileIcon, color } = getFileIcon(fileName);
    return <FileIcon size={24} className={color} />;
  };

  const getStatusIcon = (status: UploadStatus) => {
    switch (status) {
      case UploadStatus.UPLOADING:
        return <IconLoader size={16} className="animate-spin text-blue-500" />;
      case UploadStatus.SUCCESS:
        return <IconCheck size={16} className="text-green-500" />;
      case UploadStatus.ERROR:
        return <IconX size={16} className="text-red-500" />;
      case UploadStatus.CANCELLED:
        return <IconX size={16} className="text-muted-foreground" />;
      default:
        return null;
    }
  };

  const removeFile = (fileId: string) => {
    setFileUploads((prev) => {
      const upload = prev.find((u) => u.id === fileId);
      if (upload?.previewUrl) {
        URL.revokeObjectURL(upload.previewUrl);
      }
      return prev.filter((u) => u.id !== fileId);
    });
  };

  const cancelUpload = async (fileId: string) => {
    const upload = fileUploads.find((u) => u.id === fileId);
    if (!upload) return;

    if (upload.abortController) {
      upload.abortController.abort();
    }

    if (upload.objectName && upload.status === UploadStatus.UPLOADING) {
      try {
        // await deleteUploadedFile(upload.objectName);
      } catch (error) {
        console.error("Failed to delete uploaded file:", error);
      }
    }

    setFileUploads((prev) =>
      prev.map((u) => (u.id === fileId ? { ...u, status: UploadStatus.CANCELLED, abortController: undefined } : u))
    );
  };

  const uploadFile = async (fileUpload: FileUpload) => {
    const { file, id } = fileUpload;

    try {
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
          name: fileName,
          objectName: finalObjectName,
          size: file.size,
          extension: extension,
          folderId: currentFolderId,
        });
      } else {
        await axios.put(url, file, {
          headers: {
            "Content-Type": file.type,
          },
          signal: abortController.signal,
          timeout: 300000, // 5 minutes timeout for direct uploads
          maxContentLength: Infinity,
          maxBodyLength: Infinity,
          onUploadProgress: (progressEvent) => {
            const progress = (progressEvent.loaded / (progressEvent.total || file.size)) * 100;
            setFileUploads((prev) => prev.map((u) => (u.id === id ? { ...u, progress: Math.round(progress) } : u)));
          },
        });

        await registerFile({
          name: fileName,
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
  };

  const startUploads = async () => {
    const pendingUploads = fileUploads.filter((u) => u.status === UploadStatus.PENDING);

    setHasShownSuccessToast(false);

    const uploadPromises = pendingUploads.map((upload) => uploadFile(upload));
    await Promise.all(uploadPromises);

    setTimeout(() => {
      setFileUploads((currentUploads) => {
        const allComplete = currentUploads.every(
          (u) =>
            u.status === UploadStatus.SUCCESS || u.status === UploadStatus.ERROR || u.status === UploadStatus.CANCELLED
        );

        if (allComplete && !hasShownSuccessToast) {
          const successCount = currentUploads.filter((u) => u.status === UploadStatus.SUCCESS).length;
          const errorCount = currentUploads.filter((u) => u.status === UploadStatus.ERROR).length;

          if (successCount > 0) {
            // Show error toast only if there were errors, let parent handle success
            if (errorCount > 0) {
              toast.error(t("uploadFile.partialSuccess", { success: successCount, error: errorCount }));
            }
            setHasShownSuccessToast(true);
            // Call onSuccess asynchronously to avoid render-during-render
            setTimeout(() => onSuccess?.(), 0);
          }
        }

        return currentUploads;
      });
    }, 100);
  };

  const handleConfirmClose = () => {
    fileUploads.forEach((upload) => {
      if (upload.status === UploadStatus.UPLOADING && upload.abortController) {
        upload.abortController.abort();
      }
    });

    fileUploads.forEach((upload) => {
      if (upload.previewUrl) {
        URL.revokeObjectURL(upload.previewUrl);
      }
    });

    setFileUploads([]);
    setShowConfirmation(false);
    setHasShownSuccessToast(false);
    onClose();
  };

  const handleClose = () => {
    const uploadsInProgress = fileUploads.filter((u) => u.status === UploadStatus.UPLOADING).length;

    if (uploadsInProgress > 0) {
      setShowConfirmation(true);
    } else {
      handleConfirmClose();
    }
  };

  const handleContinueUploads = () => {
    setShowConfirmation(false);
  };

  const allUploadsComplete =
    fileUploads.length > 0 &&
    fileUploads.every(
      (u) => u.status === UploadStatus.SUCCESS || u.status === UploadStatus.ERROR || u.status === UploadStatus.CANCELLED
    );

  const hasUploadsInProgress = fileUploads.some((u) => u.status === UploadStatus.UPLOADING);
  const hasPendingUploads = fileUploads.some((u) => u.status === UploadStatus.PENDING);

  return (
    <>
      <Dialog open={isOpen} onOpenChange={handleClose}>
        <DialogContent
          className="sm:max-w-2xl max-h-[80vh] overflow-hidden flex flex-col"
          onPointerDownOutside={(e) => e.preventDefault()}
          onEscapeKeyDown={(e) => e.preventDefault()}
        >
          <DialogHeader>
            <DialogTitle>{t("uploadFile.multipleTitle")}</DialogTitle>
          </DialogHeader>

          <div className="flex-1 overflow-hidden flex flex-col gap-4">
            <input ref={fileInputRef} className="hidden" type="file" multiple onChange={handleFileInputChange} />

            <div
              className={`border-2 border-dashed rounded-lg p-6 cursor-pointer transition-colors ${
                isDragOver ? "border-primary bg-primary/10" : "border-border hover:border-primary/50"
              }`}
              onClick={() => fileInputRef.current?.click()}
              onDragOver={handleDragOver}
              onDragLeave={handleDragLeave}
              onDrop={handleDrop}
            >
              <div className="flex flex-col items-center gap-2">
                <IconCloudUpload size={32} className="text-muted-foreground" />
                <p className="text-foreground text-center">{t("uploadFile.selectMultipleFiles")}</p>
                <p className="text-sm text-muted-foreground">{t("uploadFile.dragAndDrop")}</p>
              </div>
            </div>

            {fileUploads.length > 0 && (
              <div className="flex-1 overflow-y-auto space-y-2 max-h-96">
                {fileUploads.map((upload) => (
                  <div key={upload.id} className="flex items-center gap-3 p-3 border rounded-lg bg-card">
                    <div className="flex-shrink-0">
                      {upload.previewUrl ? (
                        <img
                          src={upload.previewUrl}
                          alt={upload.file.name}
                          className="w-10 h-10 rounded object-cover"
                        />
                      ) : (
                        renderFileIcon(upload.file.name)
                      )}
                    </div>

                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <p className="text-sm font-medium truncate text-foreground">{upload.file.name}</p>
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
                      {upload.status === UploadStatus.UPLOADING ? (
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => cancelUpload(upload.id)}
                          className="h-8 w-8 p-0"
                        >
                          <IconX size={14} />
                        </Button>
                      ) : upload.status === UploadStatus.SUCCESS ? null : upload.status === UploadStatus.ERROR ? (
                        <div className="flex gap-1">
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => {
                              setFileUploads((prev) =>
                                prev.map((u) =>
                                  u.id === upload.id ? { ...u, status: UploadStatus.PENDING, error: undefined } : u
                                )
                              );
                            }}
                            className="h-8 w-8 p-0"
                            title={t("uploadFile.retry")}
                          >
                            <IconLoader size={14} />
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => removeFile(upload.id)}
                            className="h-8 w-8 p-0"
                          >
                            <IconTrash size={14} />
                          </Button>
                        </div>
                      ) : (
                        <Button variant="ghost" size="sm" onClick={() => removeFile(upload.id)} className="h-8 w-8 p-0">
                          <IconTrash size={14} />
                        </Button>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={handleClose}>
              {allUploadsComplete ? t("common.close") : t("common.cancel")}
            </Button>
            {!allUploadsComplete && (
              <Button
                variant="default"
                disabled={fileUploads.length === 0 || hasUploadsInProgress}
                onClick={startUploads}
              >
                {hasUploadsInProgress ? (
                  <IconLoader className="h-4 w-4 animate-spin" />
                ) : (
                  <IconCloudUpload className="h-4 w-4" />
                )}
                {hasPendingUploads ? t("uploadFile.startUploads") : t("uploadFile.upload")}
              </Button>
            )}
            {allUploadsComplete && (
              <Button variant="default" onClick={handleConfirmClose}>
                {t("uploadFile.finish")}
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmationModal
        isOpen={showConfirmation}
        onConfirm={handleConfirmClose}
        onCancel={handleContinueUploads}
        uploadsInProgress={fileUploads.filter((u) => u.status === UploadStatus.UPLOADING).length}
      />
    </>
  );
}
