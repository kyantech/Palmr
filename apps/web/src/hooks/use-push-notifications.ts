import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

interface NotificationOptions {
  title: string;
  body: string;
  icon?: string;
  badge?: string;
  tag?: string;
  requireInteraction?: boolean;
  silent?: boolean;
  data?: any;
}

export function usePushNotifications() {
  const t = useTranslations();
  const [permissionGranted, setPermissionGranted] = useState(false);
  const isSupported = useRef(typeof window !== "undefined" && "Notification" in window);

  const requestPermission = useCallback(async (): Promise<boolean> => {
    if (!isSupported.current) {
      console.warn("Push notifications are not supported in this browser");
      return false;
    }

    try {
      const permission = await Notification.requestPermission();
      const granted = permission === "granted";
      setPermissionGranted(granted);

      if (permission === "granted") {
        console.log("🔔 Push notifications enabled");
        toast.success(t("notifications.permissionGranted"));
      } else if (permission === "denied") {
        console.warn("🚫 Push notifications denied");
        toast.warning(t("notifications.permissionDenied"));
      } else {
        console.info("⏸️ Push notifications dismissed");
      }

      return granted;
    } catch (error) {
      console.error("Error requesting notification permission:", error);
      return false;
    }
  }, [t]);

  const sendNotification = useCallback(
    async (options: NotificationOptions): Promise<boolean> => {
      if (!isSupported.current) {
        console.warn("Push notifications not supported");
        return false;
      }

      if (Notification.permission !== "granted") {
        const granted = await requestPermission();
        if (!granted) return false;
      }

      try {
        const notification = new Notification(options.title, {
          body: options.body,
          icon: options.icon || "/favicon.ico",
          badge: options.badge,
          tag: options.tag,
          requireInteraction: options.requireInteraction ?? false,
          silent: options.silent ?? false,
          data: options.data,
        });

        if (!options.requireInteraction) {
          setTimeout(() => {
            notification.close();
          }, 5000);
        }

        notification.onclick = (event) => {
          event.preventDefault();
          window.focus();
          notification.close();

          if (options.data?.action === "focus-downloads") {
            const downloadIndicator = document.querySelector("[data-download-indicator]");
            if (downloadIndicator) {
              downloadIndicator.scrollIntoView({ behavior: "smooth" });
            }
          }
        };

        return true;
      } catch (error) {
        console.error("Error sending notification:", error);
        return false;
      }
    },
    [requestPermission]
  );

  useEffect(() => {
    if (isSupported.current) {
      setPermissionGranted(Notification.permission === "granted");
    }
  }, []);

  const notifyDownloadComplete = useCallback(
    async (fileName: string, fileSize?: number) => {
      const sizeText = fileSize ? ` (${(fileSize / 1024 / 1024).toFixed(1)}MB)` : "";

      return sendNotification({
        title: t("notifications.downloadComplete.title"),
        body: t("notifications.downloadComplete.body", {
          fileName: fileName + sizeText,
        }),
        icon: "/favicon.ico",
        tag: `download-complete-${Date.now()}`,
        requireInteraction: false,
        data: {
          action: "focus-downloads",
          type: "download-complete",
          fileName,
          fileSize,
        },
      });
    },
    [sendNotification, t]
  );

  const notifyDownloadFailed = useCallback(
    async (fileName: string, error?: string) => {
      return sendNotification({
        title: t("notifications.downloadFailed.title"),
        body: t("notifications.downloadFailed.body", {
          fileName,
          error: error || t("notifications.downloadFailed.unknownError"),
        }),
        icon: "/favicon.ico",
        tag: `download-failed-${Date.now()}`,
        requireInteraction: true,
        data: {
          action: "focus-downloads",
          type: "download-failed",
          fileName,
          error,
        },
      });
    },
    [sendNotification, t]
  );

  const notifyQueueProcessing = useCallback(
    async (fileName: string, position?: number) => {
      const positionText = position ? t("notifications.queueProcessing.position", { position }) : "";

      return sendNotification({
        title: t("notifications.queueProcessing.title"),
        body: t("notifications.queueProcessing.body", {
          fileName,
          position: positionText,
        }),
        icon: "/favicon.ico",
        tag: `queue-processing-${Date.now()}`,
        requireInteraction: false,
        silent: true,
        data: {
          action: "focus-downloads",
          type: "queue-processing",
          fileName,
          position,
        },
      });
    },
    [sendNotification, t]
  );

  return {
    isSupported: isSupported.current,
    hasPermission: permissionGranted,
    requestPermission,
    sendNotification,
    notifyDownloadComplete,
    notifyDownloadFailed,
    notifyQueueProcessing,
  };
}
