/**
 * TypeScript interfaces for download queue management
 */

export interface QueuedDownloadInfo {
  downloadId: string;
  position: number;
  waitTime: number;
  fileName?: string;
  fileSize?: number;
}

export interface QueueStatus {
  queueLength: number;
  maxQueueSize: number;
  activeDownloads: number;
  maxConcurrent: number;
  queuedDownloads: QueuedDownloadInfo[];
}

export interface DownloadCancelResponse {
  message: string;
  downloadId: string;
}

export interface QueueClearResponse {
  message: string;
  clearedCount: number;
}

export interface ApiResponse<T = any> {
  status: "success" | "error";
  data?: T;
  error?: string;
  message?: string;
}

export interface QueueStatusResponse extends ApiResponse<QueueStatus> {
  status: "success";
  data: QueueStatus;
}

export interface DownloadSlotRequest {
  fileName?: string;
  fileSize?: number;
  objectName: string;
}

export interface ActiveDownloadInfo {
  startTime: number;
  memoryAtStart: number;
}
