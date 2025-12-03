"use client";

import { useCallback, useState } from "react";
import { IconCheck, IconFile, IconMail, IconUpload, IconUser, IconX } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { useDropzone } from "react-dropzone";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Textarea } from "@/components/ui/textarea";
import { useUppyUpload } from "@/hooks/useUppyUpload";
import { getPresignedUrlForUploadByAlias, registerFileUploadByAlias } from "@/http/endpoints";
import { formatFileSize } from "@/utils/format-file-size";
import { UPLOAD_CONFIG } from "../constants";
import { FileUploadSectionProps } from "../types";

export function FileUploadSection({ reverseShare, password, alias, onUploadSuccess }: FileUploadSectionProps) {
  const [uploaderName, setUploaderName] = useState("");
  const [uploaderEmail, setUploaderEmail] = useState("");
  const [description, setDescription] = useState("");

  const t = useTranslations();

  const { addFiles, startUpload, removeFile, retryUpload, fileUploads, isUploading } = useUppyUpload({
    onValidate: async (file) => {
      // Client-side validations
      if (reverseShare.maxFileSize && file.size > reverseShare.maxFileSize) {
        const error = t("reverseShares.upload.errors.fileTooLarge", {
          maxSize: formatFileSize(reverseShare.maxFileSize),
        });
        toast.error(error);
        throw new Error(error);
      }

      if (reverseShare.allowedFileTypes) {
        const extension = file.name.split(".").pop()?.toLowerCase();
        const allowed = reverseShare.allowedFileTypes.split(",").map((t) => t.trim().toLowerCase());
        if (extension && !allowed.includes(extension)) {
          const error = t("reverseShares.upload.errors.fileTypeNotAllowed", {
            allowedTypes: reverseShare.allowedFileTypes,
          });
          toast.error(error);
          throw new Error(error);
        }
      }

      if (reverseShare.maxFiles) {
        const totalFiles = fileUploads.length + 1 + reverseShare.currentFileCount;
        if (totalFiles > reverseShare.maxFiles) {
          const error = t("reverseShares.upload.errors.maxFilesExceeded", {
            maxFiles: reverseShare.maxFiles,
          });
          toast.error(error);
          throw new Error(error);
        }
      }
    },
    onBeforeUpload: async (file) => {
      const timestamp = Date.now();
      const sanitizedFileName = file.name.replace(/[^a-zA-Z0-9.-]/g, "_");
      return `reverse-shares/${alias}/${timestamp}-${sanitizedFileName}`;
    },
    getPresignedUrl: async (objectName) => {
      const response = await getPresignedUrlForUploadByAlias(
        alias,
        { objectName },
        password ? { password } : undefined
      );
      return { url: response.data.url, method: "PUT" };
    },
    onAfterUpload: async (fileId, file, objectName) => {
      const fileExtension = file.name.split(".").pop() || "";

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
    },
    onSuccess: () => {
      const successCount = fileUploads.filter((u) => u.status === "success").length;

      if (successCount > 0) {
        toast.success(
          t("reverseShares.upload.success.countMessage", {
            count: successCount,
          })
        );

        onUploadSuccess?.();
      }
    },
  });

  const onDrop = useCallback(
    (acceptedFiles: File[]) => {
      addFiles(acceptedFiles);
    },
    [addFiles]
  );

  const { getRootProps, getInputProps, isDragActive } = useDropzone({
    onDrop,
    multiple: true,
    disabled: isUploading,
  });

  const validateUploadRequirements = (): boolean => {
    if (fileUploads.length === 0) {
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

  const handleUpload = async () => {
    if (!validateUploadRequirements()) return;
    startUpload();
  };

  const getCanUpload = (): boolean => {
    if (fileUploads.length === 0 || isUploading) return false;

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
  const allFilesProcessed = fileUploads.every((file) => file.status === "success" || file.status === "error");
  const hasSuccessfulUploads = fileUploads.some((file) => file.status === "success");

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
      const currentTotal = reverseShare.currentFileCount + fileUploads.length;
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

  const renderFileStatusBadge = (fileStatus: string) => {
    if (fileStatus === "success") {
      return (
        <Badge variant="default" className="bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200">
          <IconCheck className="h-3 w-3 mr-1" />
          {t("reverseShares.upload.fileList.statusUploaded")}
        </Badge>
      );
    }

    if (fileStatus === "error") {
      return <Badge variant="destructive">{t("reverseShares.upload.fileList.statusError")}</Badge>;
    }

    return null;
  };

  const renderFileItem = (upload: any) => (
    <div key={upload.id} className="flex items-center gap-3 p-3 bg-gray-50 dark:bg-gray-800 rounded-lg">
      <IconFile className="h-5 w-5 text-gray-500 flex-shrink-0" />
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium text-gray-900 dark:text-white truncate">{upload.file.name}</p>
        <p className="text-xs text-gray-500">{formatFileSize(upload.file.size)}</p>
        {upload.status === "uploading" && <Progress value={upload.progress} className="mt-2 h-2" />}
        {upload.status === "error" && upload.error && <p className="text-xs text-red-500 mt-1">{upload.error}</p>}
      </div>
      <div className="flex items-center gap-2">
        {renderFileStatusBadge(upload.status)}
        {upload.status === "pending" && (
          <Button size="sm" variant="ghost" onClick={() => removeFile(upload.id)} disabled={isUploading}>
            <IconX className="h-4 w-4" />
          </Button>
        )}
        {upload.status === "error" && (
          <div className="flex gap-1">
            <Button
              size="sm"
              variant="ghost"
              onClick={() => retryUpload(upload.id)}
              disabled={isUploading}
              title={t("reverseShares.upload.errors.retry")}
            >
              <IconUpload className="h-4 w-4" />
            </Button>
            <Button size="sm" variant="ghost" onClick={() => removeFile(upload.id)} disabled={isUploading}>
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

      {fileUploads.length > 0 && (
        <div className="space-y-2">
          <h4 className="font-medium text-gray-900 dark:text-white">{t("reverseShares.upload.fileList.title")}</h4>
          {fileUploads.map(renderFileItem)}
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
          : t("reverseShares.upload.form.uploadButton", { count: fileUploads.length })}
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
