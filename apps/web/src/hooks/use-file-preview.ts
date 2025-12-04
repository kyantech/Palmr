import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getCachedDownloadUrl, getCachedReverseShareDownloadUrl } from "@/lib/download-url-cache";
import { getFileExtension, getFileType, type FileType } from "@/utils/file-types";
import { getMimeType } from "@/utils/mime-types";

interface FilePreviewState {
  previewUrl: string | null;
  videoBlob: string | null;
  textContent: string | null;
  downloadUrl: string | null;
  isLoading: boolean;
  isLoadingPreview: boolean;
  pdfAsBlob: boolean;
  pdfLoadFailed: boolean;
}

interface UseFilePreviewProps {
  file: {
    name: string;
    objectName: string;
    type?: string;
    id?: string;
  };
  isOpen: boolean;
  isReverseShare?: boolean;
  sharePassword?: string;
}

export function useFilePreview({ file, isOpen, isReverseShare = false, sharePassword }: UseFilePreviewProps) {
  const t = useTranslations();
  const [state, setState] = useState<FilePreviewState>({
    previewUrl: null,
    videoBlob: null,
    textContent: null,
    downloadUrl: null,
    isLoading: true,
    isLoadingPreview: false,
    pdfAsBlob: false,
    pdfLoadFailed: false,
  });

  const loadedRef = useRef<string | null>(null);
  const loadingRef = useRef<boolean>(false);

  const fileType: FileType = getFileType(file.name);

  const resetState = useCallback(() => {
    setState((prev) => ({
      ...prev,
      previewUrl: null,
      videoBlob: null,
      textContent: null,
      downloadUrl: null,
      pdfAsBlob: false,
      pdfLoadFailed: false,
      isLoading: true,
    }));
    loadedRef.current = null;
    loadingRef.current = false;
  }, []);

  const cleanupBlobUrls = useCallback(() => {
    setState((prev) => {
      if (prev.previewUrl && prev.previewUrl.startsWith("blob:")) {
        URL.revokeObjectURL(prev.previewUrl);
      }
      if (prev.videoBlob && prev.videoBlob.startsWith("blob:")) {
        URL.revokeObjectURL(prev.videoBlob);
      }
      return prev;
    });
  }, []);

  const loadVideoPreview = useCallback(
    async (url: string) => {
      try {
        const response = await fetch(url);
        if (!response.ok) {
          throw new Error(`HTTP error! status: ${response.status}`);
        }

        const blob = await response.blob();
        // Create a new blob with the correct MIME type based on file extension
        // This fixes issues when the server returns application/octet-stream
        const mimeType = getMimeType(file.name);
        const typedBlob = new Blob([blob], { type: mimeType });
        const blobUrl = URL.createObjectURL(typedBlob);
        setState((prev) => ({ ...prev, videoBlob: blobUrl }));
      } catch {
        setState((prev) => ({ ...prev, previewUrl: url }));
      }
    },
    [file.name]
  );

  const loadAudioPreview = useCallback(
    async (url: string) => {
      try {
        const response = await fetch(url);
        if (!response.ok) {
          throw new Error(`HTTP error! status: ${response.status}`);
        }

        const blob = await response.blob();
        // Create a new blob with the correct MIME type based on file extension
        // This fixes issues when the server returns application/octet-stream
        const mimeType = getMimeType(file.name);
        const typedBlob = new Blob([blob], { type: mimeType });
        const blobUrl = URL.createObjectURL(typedBlob);
        setState((prev) => ({ ...prev, previewUrl: blobUrl }));
      } catch {
        setState((prev) => ({ ...prev, previewUrl: url }));
      }
    },
    [file.name]
  );

  const handlePdfLoadError = useCallback(() => {
    setState((prev) => {
      if (prev.pdfLoadFailed || prev.pdfAsBlob) return prev;
      return { ...prev, pdfLoadFailed: true };
    });
  }, []);

  const loadPdfPreview = useCallback(
    async (url: string) => {
      try {
        const response = await fetch(url);
        if (!response.ok) {
          throw new Error(`HTTP error! status: ${response.status}`);
        }

        const blob = await response.blob();
        const finalBlob = new Blob([blob], { type: "application/pdf" });
        const blobUrl = URL.createObjectURL(finalBlob);
        setState((prev) => ({
          ...prev,
          previewUrl: blobUrl,
          pdfAsBlob: true,
        }));
      } catch {
        setState((prev) => ({ ...prev, previewUrl: url }));
        setTimeout(() => {
          handlePdfLoadError();
        }, 4000);
      }
    },
    [handlePdfLoadError]
  );

  const loadTextPreview = useCallback(
    async (url: string) => {
      try {
        const response = await fetch(url);
        if (!response.ok) {
          throw new Error(`HTTP error! status: ${response.status}`);
        }

        const text = await response.text();
        const extension = getFileExtension(file.name);

        try {
          if (extension === "json") {
            const parsed = JSON.parse(text);
            const formatted = JSON.stringify(parsed, null, 2);
            setState((prev) => ({ ...prev, textContent: formatted }));
          } else {
            setState((prev) => ({ ...prev, textContent: text }));
          }
        } catch {
          setState((prev) => ({ ...prev, textContent: text }));
        }
      } catch {
        setState((prev) => ({ ...prev, textContent: null }));
      }
    },
    [file.name]
  );

  const loadPreview = useCallback(async () => {
    const fileKey = isReverseShare ? file.id : file.objectName;
    if (!fileKey || loadingRef.current) return;

    loadingRef.current = true;
    setState((prev) => ({ ...prev, isLoadingPreview: true }));

    try {
      let url: string;

      if (isReverseShare) {
        url = await getCachedReverseShareDownloadUrl(file.id!);
      } else {
        const options = sharePassword ? { headers: { "x-share-password": sharePassword } } : undefined;
        url = await getCachedDownloadUrl(file.objectName, options);
      }

      setState((prev) => ({ ...prev, downloadUrl: url }));

      switch (fileType) {
        case "video":
          await loadVideoPreview(url);
          break;
        case "audio":
          await loadAudioPreview(url);
          break;
        case "pdf":
          await loadPdfPreview(url);
          break;
        case "text":
          await loadTextPreview(url);
          break;
        default:
          setState((prev) => ({ ...prev, previewUrl: url }));
      }
    } catch {
      toast.error(t("filePreview.loadError"));
    } finally {
      setState((prev) => ({
        ...prev,
        isLoading: false,
        isLoadingPreview: false,
      }));
      loadingRef.current = false;
    }
  }, [
    isReverseShare,
    file.id,
    file.objectName,
    fileType,
    sharePassword,
    loadVideoPreview,
    loadAudioPreview,
    loadPdfPreview,
    loadTextPreview,
    t,
  ]);

  const handleDownload = useCallback(async () => {
    const fileKey = isReverseShare ? file.id : file.objectName;
    if (!fileKey) return;

    try {
      const loadingToast = toast.loading(t("filePreview.downloading") || "Downloading...");
      let url: string;
      if (isReverseShare) {
        url = await getCachedReverseShareDownloadUrl(file.id!);
      } else {
        const options = sharePassword ? { headers: { "x-share-password": sharePassword } } : undefined;
        url = await getCachedDownloadUrl(file.objectName, options);
      }

      const link = document.createElement("a");
      link.href = url;
      link.download = file.name;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);

      toast.dismiss(loadingToast);
      toast.success(t("filePreview.downloadSuccess") || "Download started");
    } catch (error) {
      console.error("Download error:", error);
      toast.error(t("filePreview.downloadError"));
    }
  }, [isReverseShare, file.id, file.objectName, file.name, sharePassword, t]);

  useEffect(() => {
    const fileKey = isReverseShare ? file.id : file.objectName;

    if (isOpen && fileKey && loadedRef.current !== fileKey) {
      loadedRef.current = fileKey;
      resetState();
      loadPreview();
    } else if (!isOpen) {
      loadedRef.current = null;
      loadingRef.current = false;
    }
  }, [isOpen, isReverseShare, file.id, file.objectName, resetState, loadPreview]);

  useEffect(() => {
    return cleanupBlobUrls;
  }, [cleanupBlobUrls]);

  useEffect(() => {
    if (!isOpen) {
      cleanupBlobUrls();
    }
  }, [isOpen, cleanupBlobUrls]);

  return {
    ...state,
    fileType,
    handleDownload,
    handlePdfLoadError,
  };
}
