import { IconDatabaseCog } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Card, CardContent } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import type { StorageUsageProps } from "../types";
import { formatStorageSize } from "../utils/format-storage-size";

export function StorageUsage({ diskSpace }: StorageUsageProps) {
  const t = useTranslations();

  return (
    <Card className="w-full">
      <CardContent className="">
        <div className="flex flex-col gap-4">
          <h2 className="text-xl font-semibold flex items-center gap-2">
            <IconDatabaseCog className="text-gray-500" size={24} />
            {t("storageUsage.title")}
          </h2>
          <div className="flex flex-col gap-2">
            <Progress
              aria-label={t("storageUsage.ariaLabel")}
              value={((diskSpace?.diskUsedGB || 0) / (diskSpace?.diskSizeGB || 1)) * 100}
              className="w-full h-3"
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
