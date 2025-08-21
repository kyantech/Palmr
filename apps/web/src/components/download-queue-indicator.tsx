"use client";

import { useEffect, useState } from "react";
import {
  IconAlertCircle,
  IconBell,
  IconBellOff,
  IconClock,
  IconDownload,
  IconLoader2,
  IconX,
} from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useDownloadQueue } from "@/hooks/use-download-queue";
import { usePushNotifications } from "@/hooks/use-push-notifications";
import { formatFileSize } from "@/utils/format-file-size";

interface PendingDownload {
  downloadId: string;
  fileName: string;
  objectName: string;
  startTime: number;
  status: "pending" | "queued" | "downloading" | "completed" | "failed";
}

interface DownloadQueueIndicatorProps {
  pendingDownloads?: PendingDownload[];
  onCancelDownload?: (downloadId: string) => void;
  className?: string;
}

export function DownloadQueueIndicator({
  pendingDownloads = [],
  onCancelDownload,
  className = "",
}: DownloadQueueIndicatorProps) {
  const t = useTranslations();

  const shouldAutoRefresh = pendingDownloads.length > 0;
  const { queueStatus, refreshQueue, cancelDownload, getEstimatedWaitTime } = useDownloadQueue(shouldAutoRefresh);
  const notifications = usePushNotifications();
  const [isOpen, setIsOpen] = useState(false);

  useEffect(() => {
    if (pendingDownloads.length > 0 || (queueStatus && queueStatus.queueLength > 0)) {
      setIsOpen(true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingDownloads.length, queueStatus?.queueLength]);

  const totalDownloads = pendingDownloads.length + (queueStatus?.queueLength || 0);
  const activeDownloads = queueStatus?.activeDownloads || 0;

  if (totalDownloads === 0 && activeDownloads === 0) {
    return null;
  }

  const getStatusIcon = (status: string) => {
    switch (status) {
      case "pending":
        return <IconLoader2 className="h-4 w-4 animate-spin text-blue-500" />;
      case "queued":
        return <IconClock className="h-4 w-4 text-yellow-500" />;
      case "downloading":
        return <IconDownload className="h-4 w-4 text-green-500" />;
      case "completed":
        return <IconDownload className="h-4 w-4 text-green-600" />;
      case "failed":
        return <IconAlertCircle className="h-4 w-4 text-red-500" />;
      default:
        return <IconLoader2 className="h-4 w-4 animate-spin" />;
    }
  };

  const getStatusText = (status: string) => {
    switch (status) {
      case "pending":
        return t("downloadQueue.status.pending");
      case "queued":
        return t("downloadQueue.status.queued");
      case "downloading":
        return t("downloadQueue.status.downloading");
      case "completed":
        return t("downloadQueue.status.completed");
      case "failed":
        return t("downloadQueue.status.failed");
      default:
        return status;
    }
  };

  return (
    <div className={`fixed bottom-6 right-6 z-50 max-w-sm ${className}`} data-download-indicator>
      <div className="flex flex-col gap-3">
        <Button
          variant="outline"
          size="sm"
          onClick={() => setIsOpen(!isOpen)}
          className="min-w-fit bg-background/80 backdrop-blur-md border-border/50 shadow-lg hover:shadow-xl transition-all duration-200 text-sm font-medium"
        >
          <IconDownload className="h-4 w-4 mr-2 text-primary" />
          Downloads
          {totalDownloads > 0 && (
            <Badge variant="secondary" className="ml-2 text-xs font-semibold bg-primary/10 text-primary border-0">
              {totalDownloads}
            </Badge>
          )}
        </Button>

        {isOpen && (
          <div className="border border-border/50 rounded-xl bg-background/95 backdrop-blur-md shadow-xl animate-in slide-in-from-bottom-2 duration-200">
            <div className="p-4 border-b border-border/50">
              <div className="flex items-center justify-between">
                <h3 className="font-semibold text-sm text-foreground">Download Manager</h3>
                <div className="flex items-center gap-2">
                  {notifications.isSupported && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={notifications.requestPermission}
                      className="h-7 w-7 p-0 rounded-md hover:bg-muted/80"
                      title={
                        notifications.hasPermission
                          ? t("notifications.permissionGranted")
                          : "Enable download notifications"
                      }
                    >
                      {notifications.hasPermission ? (
                        <IconBell className="h-3.5 w-3.5 text-green-600" />
                      ) : (
                        <IconBellOff className="h-3.5 w-3.5 text-muted-foreground" />
                      )}
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setIsOpen(false)}
                    className="h-7 w-7 p-0 rounded-md hover:bg-muted/80"
                  >
                    <IconX className="h-3.5 w-3.5 text-muted-foreground" />
                  </Button>
                </div>
              </div>

              {queueStatus && (
                <div className="mt-3 space-y-2">
                  <div className="flex items-center justify-between text-xs">
                    <span className="text-muted-foreground">Active:</span>
                    <span className="font-medium text-foreground">
                      {activeDownloads}/{queueStatus.maxConcurrent}
                    </span>
                  </div>
                  <div className="flex items-center justify-between text-xs">
                    <span className="text-muted-foreground">Queued:</span>
                    <span className="font-medium text-foreground">
                      {queueStatus.queueLength}/{queueStatus.maxQueueSize}
                    </span>
                  </div>
                  {queueStatus.maxConcurrent > 0 && (
                    <div className="space-y-1">
                      <Progress value={(activeDownloads / queueStatus.maxConcurrent) * 100} className="h-1.5" />
                      <p className="text-xs text-muted-foreground">
                        {Math.round((activeDownloads / queueStatus.maxConcurrent) * 100)}% capacity
                      </p>
                    </div>
                  )}
                </div>
              )}
            </div>

            <div className="p-3 space-y-2">
              {pendingDownloads.map((download) => (
                <div
                  key={download.downloadId}
                  className="group flex items-center justify-between p-2.5 rounded-lg bg-muted/30 hover:bg-muted/50 transition-colors border border-transparent hover:border-border/50"
                >
                  <div className="flex items-center gap-3 flex-1 min-w-0">
                    <div className="shrink-0">{getStatusIcon(download.status)}</div>
                    <div className="flex-1 min-w-0">
                      <p className="text-sm font-medium text-foreground truncate leading-tight">{download.fileName}</p>
                      <p className="text-xs text-muted-foreground mt-0.5">{getStatusText(download.status)}</p>
                    </div>
                  </div>

                  {(download.status === "pending" || download.status === "queued") && onCancelDownload && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => onCancelDownload(download.downloadId)}
                      className="h-7 w-7 p-0 opacity-0 group-hover:opacity-100 transition-opacity shrink-0 hover:bg-destructive/10 hover:text-destructive"
                    >
                      <IconX className="h-3.5 w-3.5" />
                    </Button>
                  )}
                </div>
              ))}

              {(queueStatus?.queuedDownloads || []).map((download) => {
                const waitTime = getEstimatedWaitTime(download.downloadId);

                return (
                  <div
                    key={download.downloadId}
                    className="group flex items-center justify-between p-2.5 rounded-lg bg-muted/30 hover:bg-muted/50 transition-colors border border-transparent hover:border-border/50"
                  >
                    <div className="flex items-center gap-3 flex-1 min-w-0">
                      <div className="shrink-0">
                        <IconClock className="h-4 w-4 text-amber-500" />
                      </div>
                      <div className="flex-1 min-w-0">
                        <p className="text-sm font-medium text-foreground truncate leading-tight">
                          {download.fileName || t("downloadQueue.indicator.unknownFile")}
                        </p>
                        <div className="text-xs text-muted-foreground space-y-0.5">
                          <div className="flex items-center gap-2">
                            <span>#{download.position} in queue</span>
                            {download.fileSize && (
                              <span className="text-muted-foreground/70">• {formatFileSize(download.fileSize)}</span>
                            )}
                          </div>
                          {waitTime && <p className="text-xs text-muted-foreground/80">~{waitTime} remaining</p>}
                        </div>
                      </div>
                    </div>

                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => cancelDownload(download.downloadId)}
                      className="h-7 w-7 p-0 opacity-0 group-hover:opacity-100 transition-opacity shrink-0 hover:bg-destructive/10 hover:text-destructive"
                    >
                      <IconX className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                );
              })}

              {totalDownloads === 0 && (
                <div className="text-center py-8">
                  <IconDownload className="h-8 w-8 mx-auto text-muted-foreground/50 mb-2" />
                  <p className="text-sm text-muted-foreground">No active downloads</p>
                </div>
              )}
            </div>

            {queueStatus && queueStatus.queueLength > 0 && (
              <div className="p-3 border-t border-border/50">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={refreshQueue}
                  className="w-full text-xs font-medium hover:bg-muted/80"
                >
                  Refresh Queue
                </Button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
