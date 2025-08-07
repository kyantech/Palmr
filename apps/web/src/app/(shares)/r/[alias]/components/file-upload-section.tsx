"use client";

import { useCallback, useEffect, useState } from "react";
import { IconCheck, IconFile, IconMail, IconUpload, IconUser, IconX } from "@tabler/icons-react";
import axios from "axios";
import { useTranslations } from "next-intl";
import { useDropzone } from "react-dropzone";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Textarea } from "@/components/ui/textarea";
import { getPresignedUrlForUploadByAlias, registerFileUploadByAlias } from "@/http/endpoints";
import { getSystemInfo } from "@/http/endpoints/app";
import { ChunkedUploader } from "@/utils/chunked-upload";
import { formatFileSize } from "@/utils/format-file-size";
import { FILE_STATUS, UPLOAD_CONFIG, UPLOAD_PROGRESS } from "../constants";
import { FileUploadSectionProps, FileWithProgress } from "../types";

export function FileUploadSection({ reverseShare, password, alias, onUploadSuccess }: FileUploadSectionProps) {
  const [files, setFiles] = useState<FileWithProgress[]>([]);
  const [uploaderName, setUploaderName] = useState("");
  const [uploaderEmail, setUploaderEmail] = useState("");
  const [description, setDescription] = useState("");
  const [isUploading, setIsUploading] = useState(false);
  const [isS3Enabled, setIsS3Enabled] = useState<boolean | null>(null);

  const t = useTranslations();

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

  const validateFileSize = useCallback(
    (file: File): string | null => {
      if (!reverseShare.maxFileSize) return null;

      if (file.size > reverseShare.maxFileSize) {
        return t("reverseShares.upload.errors.fileTooLarge", {
          maxSize: formatFileSize(reverseShare.maxFileSize),
        });
      }
      return null;
    },
    [reverseShare.maxFileSize, t]
  );

  const validateFileType = useCallback(
    (file: File): string | null => {
      if (!reverseShare.allowedFileTypes) return null;

      const allowedTypes = reverseShare.allowedFileTypes.split(",").map((type) => type.trim().toLowerCase());

      const fileExtension = file.name.split(".").pop()?.toLowerCase();

      if (fileExtension && !allowedTypes.includes(fileExtension)) {
        return t("reverseShares.upload.errors.fileTypeNotAllowed", {
          allowedTypes: reverseShare.allowedFileTypes,
        });
      }
      return null;
    },
    [reverseShare.allowedFileTypes, t]
  );

  const validateFileCount = useCallback((): string | null => {
    if (!reverseShare.maxFiles) return null;

    const totalFiles = files.length + 1 + reverseShare.currentFileCount;
    if (totalFiles > reverseShare.maxFiles) {
      return t("reverseShares.upload.errors.maxFilesExceeded", {
        maxFiles: reverseShare.maxFiles,
      });
    }
    return null;
  }, [reverseShare.maxFiles, reverseShare.currentFileCount, files.length, t]);

  const validateFile = useCallback(
    (file: File): string | null => {
      return validateFileSize(file) || validateFileType(file) || validateFileCount();
    },
    [validateFileSize, validateFileType, validateFileCount]
  );

  const createFileWithProgress = (file: File): FileWithProgress => ({
    file,
    progress: UPLOAD_PROGRESS.INITIAL,
    status: FILE_STATUS.PENDING,
  });

  const processAcceptedFiles = useCallback(
    (acceptedFiles: File[]): FileWithProgress[] => {
      const validFiles: FileWithProgress[] = [];

      for (const file of acceptedFiles) {
        const validationError = validateFile(file);
        if (validationError) {
          toast.error(validationError);
          continue;
        }
        validFiles.push(createFileWithProgress(file));
      }

      return validFiles;
    },
    [validateFile]
  );

  const onDrop = useCallback(
    (acceptedFiles: File[]) => {
      const newFiles = processAcceptedFiles(acceptedFiles);
      setFiles((previousFiles) => [...previousFiles, ...newFiles]);
    },
    [processAcceptedFiles]
  );

  const { getRootProps, getInputProps, isDragActive } = useDropzone({
    onDrop,
    multiple: true,
    disabled: isUploading,
  });

  const removeFile = (index: number) => {
    setFiles((previousFiles) => previousFiles.filter((_, i) => i !== index));
  };

  const updateFileStatus = (index: number, updates: Partial<FileWithProgress>) => {
    setFiles((previousFiles) => previousFiles.map((file, i) => (i === index ? { ...file, ...updates } : file)));
  };

  const generateObjectName = (fileName: string): string => {
    const timestamp = Date.now();
    return `reverse-shares/${alias}/${timestamp}-${fileName}`;
  };

  const getFileExtension = (fileName: string): string => {
    return fileName.split(".").pop() || "";
  };

  const uploadFileToStorage = async (
    file: File,
    presignedUrl: string,
    onProgress?: (progress: number) => void
  ): Promise<void> => {
    const shouldUseChunked = ChunkedUploader.shouldUseChunkedUpload(file.size, isS3Enabled ?? undefined);

    if (shouldUseChunked) {
      const chunkSize = ChunkedUploader.calculateOptimalChunkSize(file.size);

      const result = await ChunkedUploader.uploadFile({
        file,
        url: presignedUrl,
        chunkSize,
        isS3Enabled: isS3Enabled ?? undefined,
        onProgress,
      });

      if (!result.success) {
        throw new Error(result.error || "Chunked upload failed");
      }
    } else {
      await axios.put(presignedUrl, file, {
        headers: {
          "Content-Type": file.type,
        },
        onUploadProgress: (progressEvent) => {
          if (onProgress && progressEvent.total) {
            const progress = (progressEvent.loaded / progressEvent.total) * 100;
            onProgress(Math.round(progress));
          }
        },
      });
    }
  };

  const registerUploadedFile = async (file: File, objectName: string): Promise<void> => {
    const fileExtension = getFileExtension(file.name);

    await registerFileUploadByAlias(
      alias,
      {
        name: file.name,
        description: description || undefined,
        extension: fileExtension,
        size: file.size,
        objectName,
        uploaderEmail: uploaderEmail || undefined,
        uploaderName: uploaderName || undefined,
      },
      password ? { password } : undefined
    );
  };

  const uploadFile = async (fileWithProgress: FileWithProgress, index: number): Promise<void> => {
    const { file } = fileWithProgress;

    try {
      updateFileStatus(index, {
        status: FILE_STATUS.UPLOADING,
        progress: UPLOAD_PROGRESS.INITIAL,
      });

      const objectName = generateObjectName(file.name);
      const presignedResponse = await getPresignedUrlForUploadByAlias(
        alias,
        { objectName },
        password ? { password } : undefined
      );

      await uploadFileToStorage(file, presignedResponse.data.url, (progress) => {
        updateFileStatus(index, { progress });
      });

      updateFileStatus(index, { progress: UPLOAD_PROGRESS.COMPLETE });

      await registerUploadedFile(file, objectName);

      updateFileStatus(index, { status: FILE_STATUS.SUCCESS });
    } catch (error: any) {
      const errorMessage = error.response?.data?.error || t("reverseShares.upload.errors.uploadFailed");

      updateFileStatus(index, {
        status: FILE_STATUS.ERROR,
        error: errorMessage,
      });

      toast.error(errorMessage);
    }
  };

  const validateUploadRequirements = (): boolean => {
    if (files.length === 0) {
      toast.error(t("reverseShares.upload.errors.selectAtLeastOneFile"));
      return false;
    }

    const nameRequired = reverseShare.nameFieldRequired === "REQUIRED";
    const emailRequired = reverseShare.emailFieldRequired === "REQUIRED";

    if (nameRequired && !uploaderName.trim()) {
      toast.error(t("reverseShares.upload.errors.provideNameRequired"));
      return false;
    }

    if (emailRequired && !uploaderEmail.trim()) {
      toast.error(t("reverseShares.upload.errors.provideEmailRequired"));
      return false;
    }

    return true;
  };

  const processAllUploads = async (): Promise<void> => {
    const uploadPromises = files.map((fileWithProgress, index) => uploadFile(fileWithProgress, index));

    await Promise.all(uploadPromises);

    const successfulUploads = files.filter((file) => file.status === FILE_STATUS.SUCCESS);
    if (successfulUploads.length > 0) {
      toast.success(
        t("reverseShares.upload.success.countMessage", {
          count: successfulUploads.length,
        })
      );
    }
  };

  const handleUpload = async () => {
    if (!validateUploadRequirements()) return;

    setIsUploading(true);

    try {
      await processAllUploads();
    } catch {
    } finally {
      setIsUploading(false);
    }
  };

  const getCanUpload = (): boolean => {
    if (files.length === 0 || isUploading) return false;

    const nameRequired = reverseShare.nameFieldRequired === "REQUIRED";
    const emailRequired = reverseShare.emailFieldRequired === "REQUIRED";
    const nameHidden = reverseShare.nameFieldRequired === "HIDDEN";
    const emailHidden = reverseShare.emailFieldRequired === "HIDDEN";

    if (nameHidden && emailHidden) return true;

    if (nameRequired && !uploaderName.trim()) return false;

    if (emailRequired && !uploaderEmail.trim()) return false;

    return true;
  };

  const canUpload = getCanUpload();
  const allFilesProcessed = files.every(
    (file) => file.status === FILE_STATUS.SUCCESS || file.status === FILE_STATUS.ERROR
  );
  const hasSuccessfulUploads = files.some((file) => file.status === FILE_STATUS.SUCCESS);

  useEffect(() => {
    if (allFilesProcessed && hasSuccessfulUploads && files.length > 0) {
      onUploadSuccess?.();
    }
  }, [allFilesProcessed, hasSuccessfulUploads, files.length, onUploadSuccess]);

  const getDragActiveStyles = () => {
    if (isDragActive) {
      return "border-green-500 bg-blue-50 dark:bg-green-950/20";
    }
    return "border-gray-300 dark:border-gray-600 hover:border-gray-400 dark:hover:border-gray-500";
  };

  const getDropzoneStyles = () => {
    const baseStyles = "border-2 border-dashed rounded-lg p-8 text-center cursor-pointer transition-colors";
    const dragStyles = getDragActiveStyles();
    const disabledStyles = isUploading ? "opacity-50 cursor-not-allowed" : "";

    return `${baseStyles} ${dragStyles} ${disabledStyles}`.trim();
  };

  const renderFileRestrictions = () => {
    const calculateRemainingFiles = (): number => {
      if (!reverseShare.maxFiles) return 0;
      const currentTotal = reverseShare.currentFileCount + files.length;
      const remaining = reverseShare.maxFiles - currentTotal;
      return Math.max(0, remaining);
    };

    const remainingFiles = calculateRemainingFiles();

    return (
      <p className="text-sm text-gray-500 dark:text-gray-400">
        {reverseShare.allowedFileTypes && (
          <>
            {t("reverseShares.upload.fileDropzone.acceptedTypes", { types: reverseShare.allowedFileTypes })}
            <br />
          </>
        )}
        {reverseShare.maxFileSize && (
          <>
            {t("reverseShares.upload.fileDropzone.maxFileSize", { size: formatFileSize(reverseShare.maxFileSize) })}
            <br />
          </>
        )}
        {reverseShare.maxFiles && (
          <>
            {t("reverseShares.upload.fileDropzone.remainingFiles", {
              remaining: remainingFiles,
              max: reverseShare.maxFiles,
            })}
          </>
        )}
      </p>
    );
  };

  const renderFileStatusBadge = (fileWithProgress: FileWithProgress) => {
    if (fileWithProgress.status === FILE_STATUS.SUCCESS) {
      return (
        <Badge variant="default" className="bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200">
          <IconCheck className="h-3 w-3 mr-1" />
          {t("reverseShares.upload.fileList.statusUploaded")}
        </Badge>
      );
    }

    if (fileWithProgress.status === FILE_STATUS.ERROR) {
      return <Badge variant="destructive">{t("reverseShares.upload.fileList.statusError")}</Badge>;
    }

    return null;
  };

  const renderFileItem = (fileWithProgress: FileWithProgress, index: number) => (
    <div key={index} className="flex items-center gap-3 p-3 bg-gray-50 dark:bg-gray-800 rounded-lg">
      <IconFile className="h-5 w-5 text-gray-500 flex-shrink-0" />
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium text-gray-900 dark:text-white truncate">{fileWithProgress.file.name}</p>
        <p className="text-xs text-gray-500">{formatFileSize(fileWithProgress.file.size)}</p>
        {fileWithProgress.status === FILE_STATUS.UPLOADING && (
          <Progress value={fileWithProgress.progress} className="mt-2 h-2" />
        )}
        {fileWithProgress.status === FILE_STATUS.ERROR && (
          <p className="text-xs text-red-500 mt-1">{fileWithProgress.error}</p>
        )}
      </div>
      <div className="flex items-center gap-2">
        {renderFileStatusBadge(fileWithProgress)}
        {fileWithProgress.status === FILE_STATUS.PENDING && (
          <Button size="sm" variant="ghost" onClick={() => removeFile(index)} disabled={isUploading}>
            <IconX className="h-4 w-4" />
          </Button>
        )}
        {fileWithProgress.status === FILE_STATUS.ERROR && (
          <div className="flex gap-1">
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setFiles((prev) =>
                  prev.map((file, i) =>
                    i === index ? { ...file, status: FILE_STATUS.PENDING, error: undefined } : file
                  )
                );
              }}
              disabled={isUploading}
              title={t("reverseShares.upload.retry")}
            >
              <IconUpload className="h-4 w-4" />
            </Button>
            <Button size="sm" variant="ghost" onClick={() => removeFile(index)} disabled={isUploading}>
              <IconX className="h-4 w-4" />
            </Button>
          </div>
        )}
      </div>
    </div>
  );

  return (
    <div className="space-y-6">
      <div {...getRootProps()} className={getDropzoneStyles()}>
        <input {...getInputProps()} />
        <IconUpload className="mx-auto h-12 w-12 text-gray-400 mb-4" />
        <h3 className="text-lg font-medium text-gray-900 dark:text-white mb-2">
          {isDragActive
            ? t("reverseShares.upload.fileDropzone.dragActive")
            : t("reverseShares.upload.fileDropzone.dragInactive")}
        </h3>
        {renderFileRestrictions()}
      </div>

      {files.length > 0 && (
        <div className="space-y-2">
          <h4 className="font-medium text-gray-900 dark:text-white">{t("reverseShares.upload.fileList.title")}</h4>
          {files.map(renderFileItem)}
        </div>
      )}

      <div className="space-y-4">
        <div className="grid grid-cols-1 gap-4">
          {reverseShare.nameFieldRequired !== "HIDDEN" && (
            <div className="space-y-2">
              <Label htmlFor="name">
                <IconUser className="inline h-4 w-4" />
                {reverseShare.nameFieldRequired === "OPTIONAL"
                  ? t("reverseShares.upload.form.nameLabelOptional")
                  : t("reverseShares.upload.form.nameLabel")}
                {reverseShare.nameFieldRequired === "REQUIRED" && <span className="text-red-500 ml-1">*</span>}
              </Label>
              <Input
                id="name"
                placeholder={t("reverseShares.upload.form.namePlaceholder")}
                value={uploaderName}
                onChange={(e) => setUploaderName(e.target.value)}
                disabled={isUploading}
                required={reverseShare.nameFieldRequired === "REQUIRED"}
              />
            </div>
          )}
          {reverseShare.emailFieldRequired !== "HIDDEN" && (
            <div className="space-y-2">
              <Label htmlFor="email">
                <IconMail className="inline h-4 w-4" />
                {reverseShare.emailFieldRequired === "OPTIONAL"
                  ? t("reverseShares.upload.form.emailLabelOptional")
                  : t("reverseShares.upload.form.emailLabel")}
                {reverseShare.emailFieldRequired === "REQUIRED" && <span className="text-red-500 ml-1">*</span>}
              </Label>
              <Input
                id="email"
                type="email"
                placeholder={t("reverseShares.upload.form.emailPlaceholder")}
                value={uploaderEmail}
                onChange={(e) => setUploaderEmail(e.target.value)}
                disabled={isUploading}
                required={reverseShare.emailFieldRequired === "REQUIRED"}
              />
            </div>
          )}
        </div>
        <div className="space-y-2">
          <Label htmlFor="description">{t("reverseShares.upload.form.descriptionLabel")}</Label>
          <Textarea
            id="description"
            placeholder={t("reverseShares.upload.form.descriptionPlaceholder")}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            disabled={isUploading}
            rows={UPLOAD_CONFIG.TEXTAREA_ROWS}
          />
        </div>
      </div>

      <Button onClick={handleUpload} disabled={!canUpload} className="w-full text-white" size="lg" variant="default">
        {isUploading
          ? t("reverseShares.upload.form.uploading")
          : t("reverseShares.upload.form.uploadButton", { count: files.length })}
      </Button>

      {allFilesProcessed && hasSuccessfulUploads && (
        <div className="text-center p-4 bg-green-50 dark:bg-green-950/20 rounded-lg">
          <p className="text-green-800 dark:text-green-200 font-medium">{t("reverseShares.upload.success.title")}</p>
          <p className="text-sm text-green-600 dark:text-green-300 mt-1">
            {t("reverseShares.upload.success.description")}
          </p>
        </div>
      )}
    </div>
  );
}
