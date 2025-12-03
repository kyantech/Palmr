"use client";

import { IconDownload } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { EmbedCodeDisplay } from "@/components/files/embed-code-display";
import { MediaEmbedLink } from "@/components/files/media-embed-link";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useFilePreview } from "@/hooks/use-file-preview";
import { getFileIcon } from "@/utils/file-icons";
import { getFileType } from "@/utils/file-types";
import { FilePreviewRenderer } from "./previews";

interface FilePreviewModalProps {
  isOpen: boolean;
  onClose: () => void;
  file: {
    name: string;
    objectName: string;
    type?: string;
    id?: string;
    description?: string;
  };
  isReverseShare?: boolean;
  sharePassword?: string;
}

export function FilePreviewModal({
  isOpen,
  onClose,
  file,
  isReverseShare = false,
  sharePassword,
}: FilePreviewModalProps) {
  const t = useTranslations();
  const previewState = useFilePreview({ file, isOpen, isReverseShare, sharePassword });
  const fileType = getFileType(file.name);
  const isImage = fileType === "image";
  const isVideo = fileType === "video";
  const isAudio = fileType === "audio";

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="sm:max-w-3xl max-h-[90vh] overflow-hidden flex flex-col">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {(() => {
              const FileIcon = getFileIcon(file.name).icon;
              return <FileIcon size={24} />;
            })()}
            <span className="truncate">{file.name}</span>
          </DialogTitle>
          <DialogDescription className="sr-only">{t("filePreview.description")}</DialogDescription>
        </DialogHeader>
        <div className="flex-1 overflow-auto">
          <FilePreviewRenderer
            fileType={previewState.fileType}
            fileName={file.name}
            previewUrl={previewState.previewUrl}
            videoBlob={previewState.videoBlob}
            textContent={previewState.textContent}
            isLoading={previewState.isLoading}
            pdfAsBlob={previewState.pdfAsBlob}
            pdfLoadFailed={previewState.pdfLoadFailed}
            onPdfLoadError={previewState.handlePdfLoadError}
            description={file.description}
            onDownload={previewState.handleDownload}
          />
          {!isReverseShare && isImage && previewState.previewUrl && !previewState.isLoading && file.id && (
            <div className="mt-4 mb-2">
              <EmbedCodeDisplay imageUrl={previewState.previewUrl} fileName={file.name} fileId={file.id} />
            </div>
          )}
          {!isReverseShare && (isVideo || isAudio) && !previewState.isLoading && file.id && (
            <div className="mt-4 mb-2">
              <MediaEmbedLink fileId={file.id} />
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("common.close")}
          </Button>
          <Button onClick={previewState.handleDownload}>
            <IconDownload className="h-4 w-4" />
            {t("common.download")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
