import { IconPlus, IconSearch } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SharesSearchProps } from "../types";

export function SharesSearch({
  searchQuery,
  onSearchChange,
  onCreateShare,
  totalShares,
  filteredCount,
}: SharesSearchProps) {
  const t = useTranslations();

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col sm:flex-row justify-between sm:items-center gap-4">
        <h2 className="text-xl font-semibold">{t("shares.search.title")}</h2>
        <Button onClick={onCreateShare} className="w-full sm:w-auto">
          <IconPlus className="h-4 w-4" />
          {t("shares.search.createButton")}
        </Button>
      </div>

      <div className="flex items-center gap-2">
        <div className="relative max-w-xs">
          <IconSearch className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            className="pl-9"
            placeholder={t("shares.search.placeholder")}
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
          />
        </div>
        {searchQuery && (
          <span className="text-sm text-muted-foreground">
            {t("shares.search.results", {
              filtered: filteredCount,
              total: totalShares,
            })}
          </span>
        )}
      </div>
    </div>
  );
}
