"use client";

import { useCallback, useEffect, useState } from "react";
import { IconChevronDown, IconChevronUp, IconPalette } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";

const PREDEFINED_COLORS = [
  // Row 1: Standard vibrant colors
  { name: "Emerald", value: "oklch(0.59 0.18 142)" },
  { name: "Blue", value: "oklch(0.59 0.18 240)" },
  { name: "Violet", value: "oklch(0.59 0.18 270)" },
  { name: "Pink", value: "oklch(0.59 0.18 330)" },
  { name: "Red", value: "oklch(0.59 0.18 20)" },
  { name: "Orange", value: "oklch(0.59 0.18 50)" },
  { name: "Yellow", value: "oklch(0.70 0.15 90)" },
  { name: "Lime", value: "oklch(0.65 0.16 120)" },

  // Row 2: Deep colors
  { name: "Forest", value: "oklch(0.40 0.14 140)" },
  { name: "Navy", value: "oklch(0.35 0.12 240)" },
  { name: "Purple", value: "oklch(0.40 0.15 280)" },
  { name: "Crimson", value: "oklch(0.45 0.16 10)" },
  { name: "Brown", value: "oklch(0.40 0.10 40)" },
  { name: "Olive", value: "oklch(0.45 0.12 85)" },
  { name: "Teal", value: "oklch(0.50 0.14 180)" },
  { name: "Indigo", value: "oklch(0.42 0.15 260)" },

  // Row 3: Light colors
  { name: "Mint", value: "oklch(0.80 0.08 160)" },
  { name: "Sky", value: "oklch(0.75 0.10 220)" },
  { name: "Lavender", value: "oklch(0.75 0.08 290)" },
  { name: "Rose", value: "oklch(0.75 0.09 350)" },
  { name: "Coral", value: "oklch(0.75 0.10 30)" },
  { name: "Gold", value: "oklch(0.70 0.12 70)" },
  { name: "Sage", value: "oklch(0.70 0.08 130)" },
  { name: "Aqua", value: "oklch(0.75 0.09 190)" },

  // Row 4: Unique distinctive colors
  { name: "Magenta", value: "oklch(0.55 0.20 310)" },
  { name: "Cyan", value: "oklch(0.65 0.15 200)" },
  { name: "Amber", value: "oklch(0.65 0.15 60)" },
  { name: "Jade", value: "oklch(0.55 0.15 155)" },
  { name: "Slate", value: "oklch(0.50 0.02 240)" },
  { name: "Rust", value: "oklch(0.48 0.12 25)" },
  { name: "Plum", value: "oklch(0.45 0.12 300)" },
  { name: "Steel", value: "oklch(0.55 0.04 230)" },
];

const STORAGE_KEY = "palmr-custom-primary-color";

export function ColorPickerForm() {
  const t = useTranslations();
  const [selectedColor, setSelectedColor] = useState(PREDEFINED_COLORS[4].value);
  const [isCollapsed, setIsCollapsed] = useState(true);

  const applyColor = useCallback((colorValue: string) => {
    document.documentElement.style.setProperty("--primary", colorValue);
    document.documentElement.style.setProperty("--sidebar-primary", colorValue);
    document.documentElement.style.setProperty("--ring", colorValue);
    document.documentElement.style.setProperty("--sidebar-ring", colorValue);
  }, []);

  useEffect(() => {
    const savedColor = localStorage.getItem(STORAGE_KEY);
    if (savedColor) {
      setSelectedColor(savedColor);
      applyColor(savedColor);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handlePresetColorSelect = (colorValue: string) => {
    setSelectedColor(colorValue);
    applyColor(colorValue);
    localStorage.setItem(STORAGE_KEY, colorValue);
  };

  const resetToDefault = () => {
    const defaultColor = PREDEFINED_COLORS[4].value;
    handlePresetColorSelect(defaultColor);
    localStorage.removeItem(STORAGE_KEY);
  };

  return (
    <Card className="p-6 gap-0">
      <CardHeader
        className="flex flex-row items-center justify-between cursor-pointer p-0"
        onClick={() => setIsCollapsed(!isCollapsed)}
      >
        <div className="flex flex-row items-center gap-8">
          <IconPalette className="text-xl text-muted-foreground" />
          <div className="flex flex-col gap-1">
            <h2 className="text-xl font-semibold">{t("customization.colors.title")}</h2>
            <p className="text-sm text-muted-foreground">{t("customization.colors.description")}</p>
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
        <div className="flex flex-col gap-4">
          <div className="space-y-2 mb-3">
            <Label className="text-sm font-medium mb-6">{t("customization.colors.presets")}</Label>
            <div className="grid grid-cols-8 gap-3">
              {PREDEFINED_COLORS.map((color) => (
                <div key={color.name} className="flex flex-col items-center gap-1">
                  <button
                    onClick={() => handlePresetColorSelect(color.value)}
                    className={`relative w-14 h-14 rounded-xl border-2 transition-all hover:scale-105 shadow-sm ${
                      selectedColor === color.value
                        ? "border-primary ring-2 ring-primary ring-offset-2"
                        : "border-gray-200 dark:border-gray-600 hover:border-gray-300 dark:hover:border-gray-500 hover:shadow-md"
                    }`}
                    style={{
                      backgroundColor: color.value,
                    }}
                    title={color.name}
                    type="button"
                  >
                    {selectedColor === color.value && (
                      <div className="absolute inset-0 flex items-center justify-center">
                        <div className="w-4 h-4 bg-white dark:bg-black rounded-full shadow-md border border-gray-200 dark:border-gray-600" />
                      </div>
                    )}
                  </button>
                  <span className="text-xs text-muted-foreground text-center leading-tight">{color.name}</span>
                </div>
              ))}
            </div>
            <p className="text-xs text-muted-foreground ml-1 mt-6">{t("customization.colors.presetsDescription")}</p>
          </div>
        </div>
        <div className="flex justify-between items-center mt-4">
          <div className="flex"></div>
          <div className="flex">
            <Button variant="outline" onClick={resetToDefault} className="text-sm">
              {t("customization.colors.reset")}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
