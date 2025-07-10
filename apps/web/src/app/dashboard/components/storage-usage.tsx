import { IconAlertCircle, IconDatabaseCog, IconRefresh } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import type { StorageUsageProps } from "../types";
import { formatStorageSize } from "../utils/format-storage-size";

export function StorageUsage({ diskSpace, diskSpaceError, onRetry }: StorageUsageProps) {
  const t = useTranslations();

  const getErrorMessage = (error: string) => {
    switch (error) {
      case "disk_detection_failed":
        return t("storageUsage.errors.detectionFailed");
      case "server_error":
        return t("storageUsage.errors.serverError");
      default:
        return t("storageUsage.errors.unknown");
    }
  };

  if (diskSpaceError) {
    return (
      <Card className="w-full">
        <CardContent className="">
          <div className="flex flex-col gap-4">
            <div className="flex items-center justify-between">
              <h2 className="text-xl font-semibold flex items-center gap-2">
                <IconDatabaseCog className="text-gray-500" size={24} />
                {t("storageUsage.title")}
              </h2>
              <span className="text-sm text-muted-foreground">{t("storageUsage.total")}: --</span>
            </div>
            <div className="flex flex-col gap-3 py-4">
              <div className="flex items-center gap-2 text-amber-600 dark:text-amber-400">
                <IconAlertCircle size={20} />
                <span className="text-sm font-medium">{t("storageUsage.errors.title")}</span>
              </div>
              <p className="text-sm text-muted-foreground">{getErrorMessage(diskSpaceError)}</p>
              {onRetry && (
                <Button variant="outline" size="sm" onClick={onRetry} className="w-fit">
                  <IconRefresh size={16} />
                  {t("storageUsage.retry")}
                </Button>
              )}
            </div>
          </div>
        </CardContent>
      </Card>
    );
  }

  if (!diskSpace) {
    return (
      <Card className="w-full">
        <CardContent className="">
          <div className="flex flex-col gap-4">
            <div className="flex items-center justify-between">
              <h2 className="text-xl font-semibold flex items-center gap-2">
                <IconDatabaseCog className="text-gray-500" size={24} />
                {t("storageUsage.title")}
              </h2>
              <span className="text-sm text-muted-foreground">{t("storageUsage.total")}: --</span>
            </div>
            <div className="flex flex-col gap-2">
              <div className="h-3 bg-gray-200 dark:bg-gray-700 rounded animate-pulse" />
              <div className="flex justify-between text-sm text-muted-foreground">
                <span>{t("storageUsage.loading")}</span>
                <span>{t("storageUsage.loading")}</span>
              </div>
            </div>
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className="w-full">
      <CardContent className="">
        <div className="flex flex-col gap-4">
          <div className="flex items-center justify-between">
            <h2 className="text-xl font-semibold flex items-center gap-2">
              <IconDatabaseCog className="text-gray-500" size={24} />
              {t("storageUsage.title")}
            </h2>
            <span className="text-sm text-muted-foreground">
              {t("storageUsage.total")}: {formatStorageSize(diskSpace?.diskSizeGB || 0)}
            </span>
          </div>
          <div className="flex flex-col gap-2">
            <Progress
              aria-label={t("storageUsage.ariaLabel")}
              value={((diskSpace?.diskUsedGB || 0) / (diskSpace?.diskSizeGB || 1)) * 100}
              className="w-full h-2"
            />
            <div className="flex justify-between text-sm">
              <span>
                {" "}
                {formatStorageSize(diskSpace?.diskUsedGB || 0)} {t("storageUsage.used")}
              </span>
              <span>
                {" "}
                {formatStorageSize(diskSpace?.diskAvailableGB || 0)} {t("storageUsage.available")}
              </span>
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
