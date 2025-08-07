"use client";

import { useCallback, useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { getTrustedDevices, removeAllTrustedDevices, removeTrustedDevice } from "@/http/endpoints";
import type { TrustedDevice } from "@/http/endpoints/auth/trusted-devices/types";

export function useTrustedDevices() {
  const t = useTranslations();
  const [isLoading, setIsLoading] = useState(true);
  const [devices, setDevices] = useState<TrustedDevice[]>([]);
  const [isRemoveModalOpen, setIsRemoveModalOpen] = useState(false);
  const [isRemoveAllModalOpen, setIsRemoveAllModalOpen] = useState(false);
  const [deviceToRemove, setDeviceToRemove] = useState<TrustedDevice | null>(null);
  const [isRemoving, setIsRemoving] = useState(false);

  const loadDevices = useCallback(async () => {
    try {
      setIsLoading(true);
      const response = await getTrustedDevices();
      setDevices(response.devices);
    } catch (error) {
      toast.error(t("twoFactor.trustedDevices.loadFailed"));
      console.error("Failed to load trusted devices:", error);
    } finally {
      setIsLoading(false);
    }
  }, [t]);

  const handleRemoveDevice = useCallback(async (device: TrustedDevice) => {
    setDeviceToRemove(device);
    setIsRemoveModalOpen(true);
  }, []);

  const confirmRemoveDevice = useCallback(async () => {
    if (!deviceToRemove) return;

    try {
      setIsRemoving(true);
      await removeTrustedDevice({ deviceId: deviceToRemove.id });
      toast.success(t("twoFactor.trustedDevices.deviceRemoved"));
      await loadDevices();
      setIsRemoveModalOpen(false);
      setDeviceToRemove(null);
    } catch (error) {
      toast.error(t("twoFactor.trustedDevices.removeFailed"));
      console.error("Failed to remove trusted device:", error);
    } finally {
      setIsRemoving(false);
    }
  }, [deviceToRemove, t, loadDevices]);

  const handleRemoveAllDevices = useCallback(() => {
    setIsRemoveAllModalOpen(true);
  }, []);

  const confirmRemoveAllDevices = useCallback(async () => {
    try {
      setIsRemoving(true);
      await removeAllTrustedDevices();
      toast.success(t("twoFactor.trustedDevices.allDevicesRemoved"));
      await loadDevices();
      setIsRemoveAllModalOpen(false);
    } catch (error) {
      toast.error(t("twoFactor.trustedDevices.removeAllFailed"));
      console.error("Failed to remove all trusted devices:", error);
    } finally {
      setIsRemoving(false);
    }
  }, [t, loadDevices]);

  const formatDeviceName = useCallback(
    (device: TrustedDevice) => {
      const userAgent = device.userAgent;

      let deviceInfo = t("twoFactor.deviceNames.unknownDevice");

      if (!userAgent) {
        return deviceInfo;
      }

      if (userAgent.includes("Chrome")) {
        deviceInfo = t("twoFactor.deviceNames.browsers.chrome");
      } else if (userAgent.includes("Firefox")) {
        deviceInfo = t("twoFactor.deviceNames.browsers.firefox");
      } else if (userAgent.includes("Safari") && !userAgent.includes("Chrome")) {
        deviceInfo = t("twoFactor.deviceNames.browsers.safari");
      } else if (userAgent.includes("Edge")) {
        deviceInfo = t("twoFactor.deviceNames.browsers.edge");
      }

      if (userAgent.includes("Windows")) {
        deviceInfo += t("twoFactor.deviceNames.platforms.windows");
      } else if (userAgent.includes("Mac")) {
        deviceInfo += t("twoFactor.deviceNames.platforms.macos");
      } else if (userAgent.includes("Linux")) {
        deviceInfo += t("twoFactor.deviceNames.platforms.linux");
      } else if (userAgent.includes("iPhone")) {
        deviceInfo += t("twoFactor.deviceNames.platforms.iphone");
      } else if (userAgent.includes("Android")) {
        deviceInfo += t("twoFactor.deviceNames.platforms.android");
      }

      return deviceInfo;
    },
    [t]
  );

  const formatDate = useCallback((dateString: string) => {
    return new Date(dateString).toLocaleDateString("en-US", {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }, []);

  useEffect(() => {
    loadDevices();
  }, [loadDevices]);

  return {
    isLoading,
    devices,
    isRemoveModalOpen,
    isRemoveAllModalOpen,
    deviceToRemove,
    isRemoving,
    setIsRemoveModalOpen,
    setIsRemoveAllModalOpen,
    handleRemoveDevice,
    confirmRemoveDevice,
    handleRemoveAllDevices,
    confirmRemoveAllDevices,
    formatDeviceName,
    formatDate,
    loadDevices,
  };
}
