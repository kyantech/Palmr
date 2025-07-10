import { type FileType } from "@/utils/file-types";
import { AudioPreview } from "./audio-preview";
import { DefaultPreview } from "./default-preview";
import { ImagePreview } from "./image-preview";
import { PdfPreview } from "./pdf-preview";
import { TextPreview } from "./text-preview";
import { VideoPreview } from "./video-preview";

interface FilePreviewRendererProps {
  fileType: FileType;
  fileName: string;
  previewUrl: string | null;
  videoBlob: string | null;
  textContent: string | null;
  isLoading: boolean;
  pdfAsBlob: boolean;
  pdfLoadFailed: boolean;
  onPdfLoadError: () => void;
  description?: string;
  onDownload?: () => void;
}

export function FilePreviewRenderer({
  fileType,
  fileName,
  previewUrl,
  videoBlob,
  textContent,
  isLoading,
  pdfAsBlob,
  pdfLoadFailed,
  onPdfLoadError,
  description,
  onDownload,
}: FilePreviewRendererProps) {
  if (isLoading) {
    return <DefaultPreview fileName={fileName} isLoading />;
  }

  const mediaUrl = fileType === "video" ? videoBlob : previewUrl;

  if (!mediaUrl && (fileType === "video" || fileType === "audio")) {
    return <DefaultPreview fileName={fileName} />;
  }

  if (fileType === "text" && !textContent) {
    return <DefaultPreview fileName={fileName} />;
  }

  if (!previewUrl && fileType !== "video" && fileType !== "text") {
    return <DefaultPreview fileName={fileName} />;
  }

  switch (fileType) {
    case "pdf":
      return (
        <PdfPreview
          src={previewUrl!}
          fileName={fileName}
          pdfAsBlob={pdfAsBlob}
          pdfLoadFailed={pdfLoadFailed}
          onLoadError={onPdfLoadError}
        />
      );

    case "text":
      return <TextPreview content={textContent} fileName={fileName} />;

    case "image":
      return <ImagePreview src={previewUrl!} alt={fileName} description={description} onDownload={onDownload} />;

    case "audio":
      return <AudioPreview src={mediaUrl!} />;

    case "video":
      return <VideoPreview src={mediaUrl!} />;

    default:
      return <DefaultPreview fileName={fileName} />;
  }
}
