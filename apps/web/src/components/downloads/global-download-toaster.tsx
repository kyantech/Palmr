"use client";

import React from "react";
import {
  IconAlertCircle,
  IconCheck,
  IconClock,
  IconDownload,
  IconLoader,
  IconPlayerPause,
  IconRefresh,
  IconX,
} from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { DownloadStatus, useDownloadManager, type DownloadProgress } from "@/hooks/use-download-manager";
import { formatFileSize } from "@/utils/format-file-size";

/**
 * Format download speed for display
 */
function formatSpeed(bytesPerSecond: number): string {
  if (bytesPerSecond === 0) return "0 B/s";

  const units = ["B/s", "KB/s", "MB/s", "GB/s"];
  let speed = bytesPerSecond;
  let unitIndex = 0;

  while (speed >= 1024 && unitIndex < units.length - 1) {
    speed /= 1024;
    unitIndex++;
  }

  return `${speed.toFixed(1)} ${units[unitIndex]}`;
}

/**
 * Format time duration for ETA display
 */
function formatTime(seconds: number): string {
  if (seconds < 60) {
    return `${Math.round(seconds)}s`;
  } else if (seconds < 3600) {
    const minutes = Math.floor(seconds / 60);
    const remainingSeconds = Math.round(seconds % 60);
    return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
  } else {
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  }
}

/**
 * Get status icon for download
 */
function getDownloadStatusIcon(status: DownloadStatus) {
  switch (status) {
    case DownloadStatus.DOWNLOADING:
      return <IconLoader size={14} className="animate-spin text-blue-500" />;
    case DownloadStatus.COMPLETED:
      return <IconCheck size={14} className="text-green-500" />;
    case DownloadStatus.ERROR:
      return <IconAlertCircle size={14} className="text-red-500" />;
    case DownloadStatus.PAUSED:
      return <IconPlayerPause size={14} className="text-yellow-500" />;
    case DownloadStatus.CANCELLED:
      return <IconX size={14} className="text-gray-500" />;
    case DownloadStatus.PENDING:
    default:
      return <IconClock size={14} className="text-gray-500" />;
  }
}

/**
 * Individual download item component
 */
interface DownloadItemProps {
  download: DownloadProgress;
  onCancel: (downloadId: string) => void;
  onRemove: (downloadId: string) => void;
  onRetry: (downloadId: string) => void;
}

function DownloadItem({ download, onCancel, onRemove, onRetry }: DownloadItemProps) {
  const t = useTranslations();

  const getStatusText = (status: DownloadStatus) => {
    switch (status) {
      case DownloadStatus.PENDING:
        return t("downloads.status.pending");
      case DownloadStatus.DOWNLOADING:
        return t("downloads.status.downloading");
      case DownloadStatus.COMPLETED:
        return t("downloads.status.completed");
      case DownloadStatus.ERROR:
        return t("downloads.status.error");
      case DownloadStatus.PAUSED:
        return t("downloads.status.paused");
      case DownloadStatus.CANCELLED:
        return t("downloads.status.cancelled");
      default:
        return "";
    }
  };

  return (
    <div className="bg-background border rounded-lg shadow-lg p-3 flex items-center gap-3 animate-in slide-in-from-right-5 duration-300">
      {/* File icon */}
      <div className="flex-shrink-0">
        <IconDownload size={20} className="text-blue-500" />
      </div>

      {/* Main content */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <p className="text-sm font-medium truncate" title={download.fileName}>
            {download.fileName}
          </p>
          {getDownloadStatusIcon(download.status)}
        </div>

        {/* File size info */}
        <p className="text-xs text-muted-foreground">
          {download.fileSize > 0 ? (
            <>
              {formatFileSize(download.bytesDownloaded)} / {formatFileSize(download.fileSize)}
            </>
          ) : (
            formatFileSize(download.bytesDownloaded)
          )}
        </p>

        {/* Progress bar for active downloads */}
        {download.status === DownloadStatus.DOWNLOADING && (
          <div className="mt-1">
            <Progress value={download.progress} className="h-1" />
            <div className="flex justify-between text-xs text-muted-foreground mt-1">
              <span>{download.progress.toFixed(1)}%</span>
              <span>{formatSpeed(download.speed)}</span>
              {download.estimatedTimeRemaining && <span>ETA: {formatTime(download.estimatedTimeRemaining)}</span>}
            </div>
          </div>
        )}

        {/* Status text for non-downloading states */}
        {download.status !== DownloadStatus.DOWNLOADING && (
          <p className="text-xs text-muted-foreground mt-1">{getStatusText(download.status)}</p>
        )}

        {/* Error message */}
        {download.status === DownloadStatus.ERROR && download.error && (
          <p className="text-xs text-destructive mt-1" title={download.error}>
            {download.error}
          </p>
        )}
      </div>

      {/* Action buttons */}
      <div className="flex-shrink-0">
        {download.status === DownloadStatus.ERROR ? (
          <div className="flex gap-1">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onRetry(download.id)}
              className="h-6 w-6 p-0"
              title={t("downloads.retry")}
            >
              <IconRefresh size={12} />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onRemove(download.id)}
              className="h-6 w-6 p-0"
              title={t("downloads.remove")}
            >
              <IconX size={12} />
            </Button>
          </div>
        ) : download.status === DownloadStatus.COMPLETED ? (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onRemove(download.id)}
            className="h-6 w-6 p-0"
            title={t("downloads.remove")}
          >
            <IconX size={12} />
          </Button>
        ) : download.status === DownloadStatus.CANCELLED ? (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onRemove(download.id)}
            className="h-6 w-6 p-0"
            title={t("downloads.remove")}
          >
            <IconX size={12} />
          </Button>
        ) : (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onCancel(download.id)}
            className="h-6 w-6 p-0"
            title={t("downloads.cancel")}
          >
            <IconX size={12} />
          </Button>
        )}
      </div>
    </div>
  );
}

/**
 * Global download toaster component
 * Shows download progress in the bottom-right corner similar to upload toaster
 */
export function GlobalDownloadToaster() {
  const { activeDownloads, cancelDownload, removeCompletedDownload, retryDownload } = useDownloadManager();

  // Debug logs
  console.log("🎯 GlobalDownloadToaster render - activeDownloads:", activeDownloads);

  // Don't render if no active downloads
  if (activeDownloads.length === 0) {
    console.log("❌ No active downloads, not rendering toaster");
    return null;
  }

  console.log("✅ Rendering toaster with", activeDownloads.length, "downloads");

  return (
    <div className="fixed bottom-4 right-4 z-50 max-w-sm w-full space-y-2">
      {activeDownloads.map((download) => (
        <DownloadItem
          key={download.id}
          download={download}
          onCancel={cancelDownload}
          onRemove={removeCompletedDownload}
          onRetry={retryDownload}
        />
      ))}
    </div>
  );
}
