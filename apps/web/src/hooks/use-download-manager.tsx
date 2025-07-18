"use client";

import React, { createContext, ReactNode, useCallback, useContext, useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getDownloadUrl } from "@/http/endpoints";

export enum DownloadStatus {
  PENDING = "pending",
  DOWNLOADING = "downloading",
  COMPLETED = "completed",
  ERROR = "error",
  PAUSED = "paused",
  CANCELLED = "cancelled",
}

export interface DownloadProgress {
  id: string;
  fileName: string;
  fileSize: number;
  bytesDownloaded: number;
  progress: number; // 0-100
  speed: number; // bytes/sec
  status: DownloadStatus;
  startTime: number;
  estimatedTimeRemaining?: number;
  error?: string;
  objectName: string;
  abortController?: AbortController;
}

interface DownloadManagerContextType {
  activeDownloads: DownloadProgress[];
  startDownload: (objectName: string, fileName: string) => Promise<void>;
  cancelDownload: (downloadId: string) => void;
  removeCompletedDownload: (downloadId: string) => void;
  retryDownload: (downloadId: string) => Promise<void>;
  clearAllCompleted: () => void;
}

const DownloadManagerContext = createContext<DownloadManagerContextType | undefined>(undefined);

export function DownloadManagerProvider({ children }: { children: ReactNode }) {
  const t = useTranslations();
  const [activeDownloads, setActiveDownloads] = useState<DownloadProgress[]>([]);
  const downloadRefs = useRef<Map<string, { startTime: number; lastBytes: number; lastTime: number }>>(new Map());

  // Cleanup completed downloads after 5 seconds
  useEffect(() => {
    const completedDownloads = activeDownloads.filter((d) => d.status === DownloadStatus.COMPLETED);
    if (completedDownloads.length > 0) {
      const timer = setTimeout(() => {
        setActiveDownloads((prev) => prev.filter((d) => d.status !== DownloadStatus.COMPLETED));
      }, 5000);
      return () => clearTimeout(timer);
    }
  }, [activeDownloads]);

  const generateDownloadId = useCallback(() => {
    return `download_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  }, []);

  const calculateSpeed = useCallback((downloadId: string, bytesDownloaded: number): number => {
    const now = Date.now();
    const ref = downloadRefs.current.get(downloadId);
    if (!ref) {
      downloadRefs.current.set(downloadId, {
        startTime: now,
        lastBytes: bytesDownloaded,
        lastTime: now,
      });
      return 0;
    }
    const timeDiff = (now - ref.lastTime) / 1000; // seconds
    const bytesDiff = bytesDownloaded - ref.lastBytes;
    if (timeDiff > 0) {
      const speed = bytesDiff / timeDiff;
      ref.lastBytes = bytesDownloaded;
      ref.lastTime = now;
      return Math.max(0, speed);
    }
    return 0;
  }, []);

  const calculateETA = useCallback((bytesDownloaded: number, fileSize: number, speed: number): number | undefined => {
    if (speed === 0 || bytesDownloaded >= fileSize) return undefined;
    const remainingBytes = fileSize - bytesDownloaded;
    return Math.round(remainingBytes / speed); // seconds
  }, []);

  const updateDownloadProgress = useCallback(
    (downloadId: string, bytesDownloaded: number, fileSize?: number) => {
      setActiveDownloads((prev) =>
        prev.map((download) => {
          if (download.id !== downloadId) return download;
          const actualFileSize = fileSize || download.fileSize;
          const progress = actualFileSize > 0 ? (bytesDownloaded / actualFileSize) * 100 : 0;
          const speed = calculateSpeed(downloadId, bytesDownloaded);
          const estimatedTimeRemaining = calculateETA(bytesDownloaded, actualFileSize, speed);
          return {
            ...download,
            bytesDownloaded,
            fileSize: actualFileSize,
            progress: Math.min(100, Math.max(0, progress)),
            speed,
            estimatedTimeRemaining,
            status: bytesDownloaded >= actualFileSize ? DownloadStatus.COMPLETED : DownloadStatus.DOWNLOADING,
          };
        })
      );
    },
    [calculateSpeed, calculateETA]
  );

  const performDownload = useCallback(
    async (downloadId: string, objectName: string, fileName: string) => {
      try {
        const encodedObjectName = encodeURIComponent(objectName);
        const response = await getDownloadUrl(encodedObjectName);
        const downloadUrl = response.data.url;
        const abortController = new AbortController();
        setActiveDownloads((prev) =>
          prev.map((d) => (d.id === downloadId ? { ...d, abortController, status: DownloadStatus.DOWNLOADING } : d))
        );
        const downloadResponse = await fetch(downloadUrl, {
          signal: abortController.signal,
        });
        if (!downloadResponse.ok) {
          throw new Error(`HTTP ${downloadResponse.status}: ${downloadResponse.statusText}`);
        }
        const contentLength = downloadResponse.headers.get("content-length");
        const fileSize = contentLength ? parseInt(contentLength, 10) : 0;
        if (fileSize > 0) {
          setActiveDownloads((prev) => prev.map((d) => (d.id === downloadId ? { ...d, fileSize } : d)));
        }
        const reader = downloadResponse.body?.getReader();
        if (!reader) {
          throw new Error("Failed to get response reader");
        }
        const chunks: Uint8Array[] = [];
        let bytesDownloaded = 0;
        try {
          while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            if (value) {
              chunks.push(value);
              bytesDownloaded += value.length;
              updateDownloadProgress(downloadId, bytesDownloaded, fileSize);
            }
          }
          const blob = new Blob(chunks);
          const url = URL.createObjectURL(blob);
          const link = document.createElement("a");
          link.href = url;
          link.download = fileName;
          document.body.appendChild(link);
          link.click();
          document.body.removeChild(link);
          URL.revokeObjectURL(url);
          setActiveDownloads((prev) =>
            prev.map((d) =>
              d.id === downloadId
                ? {
                    ...d,
                    status: DownloadStatus.COMPLETED,
                    progress: 100,
                    abortController: undefined,
                  }
                : d
            )
          );
          downloadRefs.current.delete(downloadId);
          toast.success(t("files.downloadSuccess", { fileName }));
        } catch (error: any) {
          if (error.name === "AbortError") {
            setActiveDownloads((prev) =>
              prev.map((d) =>
                d.id === downloadId ? { ...d, status: DownloadStatus.CANCELLED, abortController: undefined } : d
              )
            );
          } else {
            throw error;
          }
        } finally {
          reader.releaseLock();
        }
      } catch (error: any) {
        console.error(`Download failed for ${fileName}:`, error);
        setActiveDownloads((prev) =>
          prev.map((d) =>
            d.id === downloadId
              ? {
                  ...d,
                  status: DownloadStatus.ERROR,
                  error: error.message || t("files.downloadError"),
                  abortController: undefined,
                }
              : d
          )
        );
        downloadRefs.current.delete(downloadId);
      }
    },
    [updateDownloadProgress, t]
  );

  const startDownload = useCallback(
    async (objectName: string, fileName: string) => {
      console.log("🚀 Starting download:", { objectName, fileName });
      const downloadId = generateDownloadId();
      const newDownload: DownloadProgress = {
        id: downloadId,
        fileName,
        fileSize: 0,
        bytesDownloaded: 0,
        progress: 0,
        speed: 0,
        status: DownloadStatus.PENDING,
        startTime: Date.now(),
        objectName,
      };
      setActiveDownloads((prev) => [...prev, newDownload]);
      await performDownload(downloadId, objectName, fileName);
    },
    [performDownload, generateDownloadId]
  );

  const cancelDownload = useCallback((downloadId: string) => {
    setActiveDownloads((prev) =>
      prev.map((d) => {
        if (d.id === downloadId && d.abortController) {
          d.abortController.abort();
          return { ...d, status: DownloadStatus.CANCELLED, abortController: undefined };
        }
        return d;
      })
    );
  }, []);

  const removeCompletedDownload = useCallback((downloadId: string) => {
    setActiveDownloads((prev) => prev.filter((d) => d.id !== downloadId));
  }, []);

  const retryDownload = useCallback(
    async (downloadId: string) => {
      const download = activeDownloads.find((d) => d.id === downloadId);
      if (!download) return;
      setActiveDownloads((prev) =>
        prev.map((d) => (d.id === downloadId ? { ...d, status: DownloadStatus.PENDING, error: undefined } : d))
      );
      await performDownload(downloadId, download.objectName, download.fileName);
    },
    [activeDownloads, performDownload]
  );

  const clearAllCompleted = useCallback(() => {
    setActiveDownloads((prev) => prev.filter((d) => d.status !== DownloadStatus.COMPLETED));
  }, []);

  const value: DownloadManagerContextType = {
    activeDownloads,
    startDownload,
    cancelDownload,
    removeCompletedDownload,
    retryDownload,
    clearAllCompleted,
  };

  return <DownloadManagerContext.Provider value={value}>{children}</DownloadManagerContext.Provider>;
}

export function useDownloadManager(): DownloadManagerContextType {
  const ctx = useContext(DownloadManagerContext);
  if (!ctx) {
    throw new Error("useDownloadManager must be used within a DownloadManagerProvider");
  }
  return ctx;
}
