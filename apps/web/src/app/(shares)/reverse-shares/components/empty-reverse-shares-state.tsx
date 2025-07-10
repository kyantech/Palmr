import { IconPlus, IconUpload } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";

interface EmptyReverseSharesStateProps {
  onCreateReverseShare: () => void;
}

export function EmptyReverseSharesState({ onCreateReverseShare }: EmptyReverseSharesStateProps) {
  const t = useTranslations();

  return (
    <div className="flex flex-col items-center justify-center py-12 text-center">
      <div className="mb-6">
        <div className="mx-auto w-20 h-20 bg-muted rounded-full flex items-center justify-center">
          <IconUpload className="h-10 w-10 text-muted-foreground" />
        </div>
      </div>

      <div className="space-y-2 mb-6">
        <h3 className="text-lg font-semibold">{t("reverseShares.empty.title")}</h3>
        <p className="text-muted-foreground max-w-md">{t("reverseShares.empty.description")}</p>
      </div>

      <Button onClick={onCreateReverseShare}>
        <IconPlus className="h-4 w-4" />
        {t("reverseShares.empty.createButton")}
      </Button>
    </div>
  );
}
