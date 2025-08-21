import type { AxiosRequestConfig } from "axios";

import apiInstance from "@/config/api";

export interface QueuedDownload {
  downloadId: string;
  position: number;
  waitTime: number;
  fileName?: string;
  fileSize?: number;
}

export interface DownloadQueueStatus {
  queueLength: number;
  maxQueueSize: number;
  activeDownloads: number;
  maxConcurrent: number;
  queuedDownloads: QueuedDownload[];
}

export interface DownloadQueueStatusResult {
  status: string;
  data: DownloadQueueStatus;
}

export interface CancelDownloadResult {
  message: string;
  downloadId: string;
}

export interface ClearQueueResult {
  message: string;
  clearedCount: number;
}

/**
 * Get current download queue status
 * @summary Get Download Queue Status
 */
export const getDownloadQueueStatus = <TData = DownloadQueueStatusResult>(
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/filesystem/download-queue/status`, options);
};

/**
 * Cancel a specific queued download
 * @summary Cancel Queued Download
 */
export const cancelQueuedDownload = <TData = CancelDownloadResult>(
  downloadId: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.delete(`/api/filesystem/download-queue/${downloadId}`, options);
};

/**
 * Clear the entire download queue (admin operation)
 * @summary Clear Download Queue
 */
export const clearDownloadQueue = <TData = ClearQueueResult>(options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.delete(`/api/filesystem/download-queue`, options);
};
