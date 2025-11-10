"use client";

import { useCallback, useEffect, useState } from "react";
import { IconChevronDown, IconChevronUp, IconDeviceLaptop } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";

const BACKGROUND_OPTIONS = {
  light: [
    { name: "Default", background: "oklch(0.9911 0 0)", description: "Pure white" },
    { name: "Warm", background: "oklch(0.99 0.005 85)", description: "Slightly warm tone" },
    { name: "Cool", background: "oklch(0.99 0.005 230)", description: "Slightly cool tone" },
  ],
  dark: [
    { name: "Default", background: "oklch(0.15 0 0)", description: "Standard dark" },
    { name: "Darker", background: "oklch(0.13 0 0)", description: "Darker gray" },
    { name: "Pure Black", background: "oklch(0 0 0)", description: "True black" },
  ],
};

const STORAGE_KEY = "palmr-custom-background";

export function BackgroundPickerForm() {
  const t = useTranslations();
  const [selectedBackground, setSelectedBackground] = useState({
    light: BACKGROUND_OPTIONS.light[0].background,
    dark: BACKGROUND_OPTIONS.dark[0].background,
  });
  const [isCollapsed, setIsCollapsed] = useState(true);
  const applyBackground = useCallback((backgroundValues: { light: string; dark: string }) => {
    document.documentElement.style.setProperty("--custom-background-light", backgroundValues.light);
    document.documentElement.style.setProperty("--custom-background-dark", backgroundValues.dark);

    console.log("Applied background:", backgroundValues);
  }, []);

  useEffect(() => {
    const savedBackground = localStorage.getItem(STORAGE_KEY);
    if (savedBackground) {
      const parsed = JSON.parse(savedBackground);
      setSelectedBackground(parsed);
      applyBackground(parsed);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleBackgroundSelect = (mode: "light" | "dark", backgroundValue: string) => {
    const newBackground = {
      ...selectedBackground,
      [mode]: backgroundValue,
    };
    setSelectedBackground(newBackground);
    applyBackground(newBackground);
    localStorage.setItem(STORAGE_KEY, JSON.stringify(newBackground));
  };

  const resetToDefault = () => {
    const defaultBackground = {
      light: BACKGROUND_OPTIONS.light[0].background,
      dark: BACKGROUND_OPTIONS.dark[0].background,
    };
    setSelectedBackground(defaultBackground);
    applyBackground(defaultBackground);
    localStorage.removeItem(STORAGE_KEY);
  };

  return (
    <Card className="p-6 gap-0">
      <CardHeader
        className="flex flex-row items-center justify-between cursor-pointer p-0"
        onClick={() => setIsCollapsed(!isCollapsed)}
      >
        <div className="flex flex-row items-center gap-8">
          <IconDeviceLaptop className="text-xl text-muted-foreground" />
          <div className="flex flex-col gap-1">
            <h2 className="text-xl font-semibold">{t("customization.background.title")}</h2>
            <p className="text-sm text-muted-foreground">{t("customization.background.description")}</p>
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
            <Label className="text-sm font-medium mb-6">{t("customization.background.lightMode")}</Label>
            <div className="grid grid-cols-3 gap-4">
              {BACKGROUND_OPTIONS.light.map((bg) => (
                <button
                  key={bg.name}
                  onClick={() => handleBackgroundSelect("light", bg.background)}
                  className={`relative p-4 rounded-xl border-2 transition-all hover:shadow-md text-center group ${
                    selectedBackground.light === bg.background
                      ? "border-primary ring-2 ring-primary ring-offset-2 bg-primary/5"
                      : "border-gray-200 dark:border-gray-600 hover:border-gray-300 dark:hover:border-gray-500 hover:bg-muted/30"
                  }`}
                  type="button"
                >
                  <div className="flex flex-col items-center gap-3">
                    <div className="flex flex-col items-center gap-2">
                      <span className="font-medium text-base">{bg.name}</span>
                      <span className="text-xs text-muted-foreground">{bg.description}</span>
                    </div>
                    <div
                      className="w-12 h-8 rounded border border-gray-300"
                      style={{ backgroundColor: bg.background }}
                    />
                  </div>
                </button>
              ))}
            </div>
          </div>

          <div className="space-y-2 mb-3">
            <Label className="text-sm font-medium mb-6">{t("customization.background.darkMode")}</Label>
            <div className="grid grid-cols-3 gap-4">
              {BACKGROUND_OPTIONS.dark.map((bg) => (
                <button
                  key={bg.name}
                  onClick={() => handleBackgroundSelect("dark", bg.background)}
                  className={`relative p-4 rounded-xl border-2 transition-all hover:shadow-md text-center group ${
                    selectedBackground.dark === bg.background
                      ? "border-primary ring-2 ring-primary ring-offset-2 bg-primary/5"
                      : "border-gray-200 dark:border-gray-600 hover:border-gray-300 dark:hover:border-gray-500 hover:bg-muted/30"
                  }`}
                  type="button"
                >
                  <div className="flex flex-col items-center gap-3">
                    <div className="flex flex-col items-center gap-2">
                      <span className="font-medium text-base">{bg.name}</span>
                      <span className="text-xs text-muted-foreground">{bg.description}</span>
                    </div>
                    <div
                      className="w-12 h-8 rounded border border-gray-600"
                      style={{ backgroundColor: bg.background }}
                    />
                  </div>
                </button>
              ))}
            </div>
          </div>

          <p className="text-xs text-muted-foreground ml-1">{t("customization.background.availableDescription")}</p>
        </div>
        <div className="flex justify-between items-center mt-4">
          <div className="flex"></div>
          <div className="flex">
            <Button variant="outline" onClick={resetToDefault} className="text-sm">
              {t("customization.background.reset")}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
