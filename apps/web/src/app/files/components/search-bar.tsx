import { IconSearch } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Input } from "@/components/ui/input";
import type { SearchBarProps } from "../types";

export function SearchBar({
  searchQuery,
  onSearch,
  totalFiles,
  totalFolders = 0,
  filteredCount,
  filteredFolders = 0,
}: SearchBarProps) {
  const t = useTranslations();

  const totalItems = totalFiles + totalFolders;
  const filteredItems = filteredCount + filteredFolders;

  return (
    <div className="flex items-center gap-2">
      <div className="relative max-w-xs">
        <IconSearch className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
        <Input
          className="pl-9"
          placeholder={t("searchBar.placeholder")}
          value={searchQuery}
          onChange={(e) => onSearch(e.target.value)}
        />
      </div>
      {searchQuery && (
        <span className="text-sm text-muted-foreground">
          {t("searchBar.results", { filtered: filteredItems, total: totalItems })}
        </span>
      )}
    </div>
  );
}
