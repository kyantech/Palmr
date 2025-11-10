import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import {
  cancelQueuedDownload,
  getDownloadQueueStatus,
  type DownloadQueueStatus,
} from "@/http/endpoints/download-queue";

export interface DownloadQueueHook {
  queueStatus: DownloadQueueStatus | null;
  isLoading: boolean;
  error: string | null;
  refreshQueue: () => Promise<void>;
  cancelDownload: (downloadId: string) => Promise<void>;
  getQueuePosition: (downloadId: string) => number | null;
  isDownloadQueued: (downloadId: string) => boolean;
  getEstimatedWaitTime: (downloadId: string) => string | null;
}

export function useDownloadQueue(autoRefresh = true, initialIntervalMs = 3000) {
  const t = useTranslations();
  const [queueStatus, setQueueStatus] = useState<DownloadQueueStatus | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [currentInterval, setCurrentInterval] = useState(initialIntervalMs);
  const [noActivityCount, setNoActivityCount] = useState(0);

  const refreshQueue = useCallback(async () => {
    try {
      setIsLoading(true);
      setError(null);
      const response = await getDownloadQueueStatus();
      const newStatus = response.data;

      const hasActivity = newStatus.activeDownloads > 0 || newStatus.queueLength > 0;
      const previousActivity = (queueStatus?.activeDownloads || 0) > 0 || (queueStatus?.queueLength || 0) > 0;
      const statusChanged = JSON.stringify(queueStatus) !== JSON.stringify(newStatus);

      if (!hasActivity && !previousActivity && !statusChanged) {
        setNoActivityCount((prev) => prev + 1);
      } else {
        setNoActivityCount(0);
        setCurrentInterval(initialIntervalMs);
      }

      setQueueStatus(newStatus);
    } catch (err: any) {
      const errorMessage = err?.response?.data?.error || err?.message || "Failed to fetch queue status";
      setError(errorMessage);
      console.error("Error fetching download queue status:", err);
    } finally {
      setIsLoading(false);
    }
  }, [queueStatus, initialIntervalMs]);

  const cancelDownload = useCallback(
    async (downloadId: string) => {
      try {
        await cancelQueuedDownload(downloadId);
        toast.success(t("downloadQueue.cancelSuccess"));
        await refreshQueue();
      } catch (err: any) {
        const errorMessage = err?.response?.data?.error || err?.message || "Failed to cancel download";
        toast.error(t("downloadQueue.cancelError", { error: errorMessage }));
        console.error("Error cancelling download:", err);
      }
    },
    [refreshQueue, t]
  );

  const getQueuePosition = useCallback(
    (downloadId: string): number | null => {
      if (!queueStatus) return null;
      const download = queueStatus.queuedDownloads.find((d) => d.downloadId === downloadId);
      return download?.position || null;
    },
    [queueStatus]
  );

  const isDownloadQueued = useCallback(
    (downloadId: string): boolean => {
      if (!queueStatus) return false;
      return queueStatus.queuedDownloads.some((d) => d.downloadId === downloadId);
    },
    [queueStatus]
  );

  const getEstimatedWaitTime = useCallback(
    (downloadId: string): string | null => {
      if (!queueStatus) return null;

      const download = queueStatus.queuedDownloads.find((d) => d.downloadId === downloadId);
      if (!download) return null;

      const waitTimeMs = download.waitTime;
      const waitTimeSeconds = Math.floor(waitTimeMs / 1000);

      if (waitTimeSeconds < 60) {
        return t("downloadQueue.waitTime.seconds", { seconds: waitTimeSeconds });
      } else if (waitTimeSeconds < 3600) {
        const minutes = Math.floor(waitTimeSeconds / 60);
        return t("downloadQueue.waitTime.minutes", { minutes });
      } else {
        const hours = Math.floor(waitTimeSeconds / 3600);
        const minutes = Math.floor((waitTimeSeconds % 3600) / 60);
        return t("downloadQueue.waitTime.hoursMinutes", { hours, minutes });
      }
    },
    [queueStatus, t]
  );

  useEffect(() => {
    if (!autoRefresh) return;

    let actualInterval = currentInterval;

    if (noActivityCount > 5) {
      console.log("[DOWNLOAD QUEUE] No activity detected, stopping polling");
      return;
    } else if (noActivityCount > 2) {
      actualInterval = 10000;
      setCurrentInterval(10000);
    }

    refreshQueue();

    const interval = setInterval(refreshQueue, actualInterval);

    return () => clearInterval(interval);
  }, [autoRefresh, refreshQueue, currentInterval, noActivityCount]);

  return {
    queueStatus,
    isLoading,
    error,
    refreshQueue,
    cancelDownload,
    getQueuePosition,
    isDownloadQueued,
    getEstimatedWaitTime,
  };
}
