import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getDownloadUrl } from "@/http/endpoints";
import { downloadReverseShareFile } from "@/http/endpoints/reverse-shares";
import { getFileExtension, getFileType, type FileType } from "@/utils/file-types";

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

  const loadVideoPreview = useCallback(async (url: string) => {
    try {
      const response = await fetch(url);
      if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`);
      }

      const blob = await response.blob();
      const blobUrl = URL.createObjectURL(blob);
      setState((prev) => ({ ...prev, videoBlob: blobUrl }));
    } catch {
      setState((prev) => ({ ...prev, previewUrl: url }));
    }
  }, []);

  const loadAudioPreview = useCallback(async (url: string) => {
    try {
      const response = await fetch(url);
      if (!response.ok) {
        throw new Error(`HTTP error! status: ${response.status}`);
      }

      const blob = await response.blob();
      const blobUrl = URL.createObjectURL(blob);
      setState((prev) => ({ ...prev, previewUrl: blobUrl }));
    } catch {
      setState((prev) => ({ ...prev, previewUrl: url }));
    }
  }, []);

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
        const response = await downloadReverseShareFile(file.id!);
        url = response.data.url;
      } else {
        const encodedObjectName = encodeURIComponent(file.objectName);
        const params: Record<string, string> = {};
        if (sharePassword) params.password = sharePassword;

        const response = await getDownloadUrl(
          encodedObjectName,
          Object.keys(params).length > 0
            ? {
                params: { ...params },
              }
            : undefined
        );
        url = response.data.url;
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
      if (isReverseShare) {
        const response = await downloadReverseShareFile(file.id!);

        // Direct S3 download
        const link = document.createElement("a");
        link.href = response.data.url;
        link.download = file.name;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);

        toast.success(t("filePreview.downloadSuccess"));
      } else {
        const encodedObjectName = encodeURIComponent(file.objectName);
        const params: Record<string, string> = {};
        if (sharePassword) params.password = sharePassword;

        const response = await getDownloadUrl(
          encodedObjectName,
          Object.keys(params).length > 0 ? { params } : undefined
        );

        // Direct S3 download
        const link = document.createElement("a");
        link.href = response.data.url;
        link.download = file.name;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);

        toast.success(t("filePreview.downloadSuccess"));
      }
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
