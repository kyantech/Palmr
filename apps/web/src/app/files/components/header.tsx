import { IconCloudUpload, IconFolderPlus } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import type { HeaderProps } from "../types";

export function Header({ onUpload, onCreateFolder }: HeaderProps) {
  const t = useTranslations();

  return (
    <div className="flex flex-col sm:flex-row justify-between sm:items-center gap-4">
      <h2 className="text-xl font-semibold">{t("files.title")}</h2>
      <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
        {onCreateFolder && (
          <Button variant="outline" onClick={onCreateFolder} className="w-full sm:w-auto">
            <IconFolderPlus className="h-4 w-4" />
            {t("folderActions.createFolder")}
          </Button>
        )}
        <Button variant="default" onClick={onUpload} className="w-full sm:w-auto">
          <IconCloudUpload className="h-4 w-4" />
          {t("files.uploadFile")}
        </Button>
      </div>
    </div>
  );
}
