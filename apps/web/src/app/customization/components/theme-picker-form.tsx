"use client";

import { useState } from "react";
import { IconChevronDown, IconChevronUp, IconDeviceLaptop, IconMoon, IconSun, IconSunMoon } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { useTheme } from "next-themes";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";

const THEME_OPTIONS = [
  { nameKey: "theme.system", value: "system", icon: IconDeviceLaptop, descriptionKey: "theme.systemDescription" },
  { nameKey: "theme.light", value: "light", icon: IconSun, descriptionKey: "theme.lightDescription" },
  { nameKey: "theme.dark", value: "dark", icon: IconMoon, descriptionKey: "theme.darkDescription" },
];

export function ThemePickerForm() {
  const t = useTranslations();
  const { theme, setTheme } = useTheme();
  const [isCollapsed, setIsCollapsed] = useState(true);

  const handleThemeSelect = (themeValue: string) => {
    setTheme(themeValue);
  };

  const resetToDefault = () => {
    setTheme("system");
  };

  return (
    <Card className="p-6 gap-0">
      <CardHeader
        className="flex flex-row items-center justify-between cursor-pointer p-0"
        onClick={() => setIsCollapsed(!isCollapsed)}
      >
        <div className="flex flex-row items-center gap-8">
          <IconSunMoon className="text-xl text-muted-foreground" />
          <div className="flex flex-col gap-1">
            <h2 className="text-xl font-semibold">{t("customization.theme.title")}</h2>
            <p className="text-sm text-muted-foreground">{t("customization.theme.description")}</p>
          </div>
        </div>
        {isCollapsed ? (
          <IconChevronDown className="text-muted-foreground" />
        ) : (
          <IconChevronUp className="text-muted-foreground" />
        )}
      </CardHeader>
      <CardContent className={`${isCollapsed ? "hidden" : "block"} px-0`}>
        <Separator className="my-6" />
        <div className="flex flex-col gap-6">
          <div className="space-y-2 mb-3">
            <Label className="text-sm font-medium mb-6">{t("customization.theme.selectTheme")}</Label>
            <div className="grid grid-cols-3 gap-4">
              {THEME_OPTIONS.map((themeOption) => {
                const IconComponent = themeOption.icon;
                return (
                  <button
                    key={themeOption.value}
                    onClick={() => handleThemeSelect(themeOption.value)}
                    className={`relative p-4 rounded-xl border-2 transition-all hover:shadow-md text-center group ${
                      theme === themeOption.value
                        ? "border-primary ring-2 ring-primary ring-offset-2 bg-primary/5"
                        : "border-gray-200 dark:border-gray-600 hover:border-gray-300 dark:hover:border-gray-500 hover:bg-muted/30"
                    }`}
                    type="button"
                  >
                    <div className="flex flex-col items-center gap-3">
                      <div className="flex flex-col items-center gap-2">
                        <IconComponent className="w-6 h-6 text-muted-foreground" />
                        <span className="font-medium text-base">{t(themeOption.nameKey)}</span>
                        <span className="text-xs text-muted-foreground">{t(themeOption.descriptionKey)}</span>
                      </div>
                    </div>
                  </button>
                );
              })}
            </div>
          </div>

          <p className="text-xs text-muted-foreground ml-1 mt-6">{t("customization.theme.availableDescription")}</p>
        </div>
        <div className="flex justify-between items-center mt-4">
          <div className="flex"></div>
          <div className="flex">
            <Button variant="outline" onClick={resetToDefault} className="text-sm">
              {t("customization.theme.reset")}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
