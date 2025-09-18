"use client";

import { useCallback, useEffect, useState } from "react";
import { IconPalette } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const PREDEFINED_COLORS = [
  { name: "Green", value: "oklch(0.5927 0.1767 142.49)" },
  { name: "Blue", value: "oklch(0.5927 0.1767 240)" },
  { name: "Purple", value: "oklch(0.5927 0.1767 280)" },
  { name: "Pink", value: "oklch(0.5927 0.1767 330)" },
  { name: "Red", value: "oklch(0.5927 0.1767 20)" },
  { name: "Orange", value: "oklch(0.5927 0.1767 60)" },
  { name: "Yellow", value: "oklch(0.5927 0.1767 100)" },
  { name: "Teal", value: "oklch(0.5927 0.1767 180)" },
];

const STORAGE_KEY = "palmr-custom-primary-color";

function hexToOklch(hex: string): string {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;

  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const diff = max - min;

  let hue = 0;
  if (diff !== 0) {
    if (max === r) {
      hue = ((g - b) / diff) % 6;
    } else if (max === g) {
      hue = (b - r) / diff + 2;
    } else {
      hue = (r - g) / diff + 4;
    }
  }
  hue = hue * 60;
  if (hue < 0) hue += 360;

  return `oklch(0.5927 0.1767 ${hue})`;
}

function oklchToHex(oklch: string): string {
  const hueMatch = oklch.match(/oklch\([^)]*\s([^)]*)\)/);
  if (!hueMatch) return "#22c55e";

  const hue = parseFloat(hueMatch[1]);

  const c = 1;
  const x = c * (1 - Math.abs(((hue / 60) % 2) - 1));
  const m = 0.5 - c / 2;

  let r, g, b;

  if (hue >= 0 && hue < 60) {
    [r, g, b] = [c, x, 0];
  } else if (hue >= 60 && hue < 120) {
    [r, g, b] = [x, c, 0];
  } else if (hue >= 120 && hue < 180) {
    [r, g, b] = [0, c, x];
  } else if (hue >= 180 && hue < 240) {
    [r, g, b] = [0, x, c];
  } else if (hue >= 240 && hue < 300) {
    [r, g, b] = [x, 0, c];
  } else {
    [r, g, b] = [c, 0, x];
  }

  r = Math.round((r + m) * 255);
  g = Math.round((g + m) * 255);
  b = Math.round((b + m) * 255);

  return `#${r.toString(16).padStart(2, "0")}${g.toString(16).padStart(2, "0")}${b.toString(16).padStart(2, "0")}`;
}

export function ColorPickerForm() {
  const t = useTranslations();
  const [selectedColor, setSelectedColor] = useState(PREDEFINED_COLORS[4].value);
  const [customColor, setCustomColor] = useState("#22c55e");

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
      setCustomColor(oklchToHex(savedColor));
      applyColor(savedColor);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handlePresetColorSelect = (colorValue: string) => {
    setSelectedColor(colorValue);
    setCustomColor(oklchToHex(colorValue));
    applyColor(colorValue);
    localStorage.setItem(STORAGE_KEY, colorValue);
  };

  const handleCustomColorChange = (hexColor: string) => {
    setCustomColor(hexColor);
    const oklchColor = hexToOklch(hexColor);
    setSelectedColor(oklchColor);
    applyColor(oklchColor);
    localStorage.setItem(STORAGE_KEY, oklchColor);
  };

  const resetToDefault = () => {
    const defaultColor = PREDEFINED_COLORS[0].value;
    handlePresetColorSelect(defaultColor);
    localStorage.removeItem(STORAGE_KEY);
  };

  return (
    <Card>
      <CardHeader>
        <h2 className="text-lg font-semibold flex items-center gap-2">
          <IconPalette className="w-5 h-5" />
          {t("profile.colors.title")}
        </h2>
        <p className="text-sm text-muted-foreground">{t("profile.colors.description")}</p>
      </CardHeader>
      <CardContent className="space-y-6">
        <div>
          <Label className="text-sm font-medium mb-3 block">{t("profile.colors.presets")}</Label>
          <div className="grid grid-cols-4 gap-2">
            {PREDEFINED_COLORS.map((color) => (
              <button
                key={color.name}
                onClick={() => handlePresetColorSelect(color.value)}
                className={`relative w-12 h-12 rounded-lg border-2 transition-all hover:scale-105 ${
                  selectedColor === color.value
                    ? "border-current ring-2 ring-current ring-offset-2"
                    : "border-gray-200 dark:border-gray-600 hover:border-gray-300 dark:hover:border-gray-500"
                }`}
                style={{
                  backgroundColor: `color(srgb ${color.value})`,
                  background: `linear-gradient(135deg, ${color.value}, ${color.value})`,
                }}
                title={color.name}
                type="button"
              >
                {selectedColor === color.value && (
                  <div className="absolute inset-0 flex items-center justify-center">
                    <div className="w-3 h-3 bg-white dark:bg-black rounded-full shadow-sm" />
                  </div>
                )}
              </button>
            ))}
          </div>
        </div>

        <div>
          <Label htmlFor="custom-color" className="text-sm font-medium mb-3 block">
            {t("profile.colors.custom")}
          </Label>
          <div className="flex items-center gap-3">
            <Input
              id="custom-color"
              type="color"
              value={customColor}
              onChange={(e) => handleCustomColorChange(e.target.value)}
              className="w-16 h-10 p-1 rounded-lg border-2 cursor-pointer"
            />
            <Input
              value={customColor.toUpperCase()}
              onChange={(e) => handleCustomColorChange(e.target.value)}
              placeholder="#22C55E"
              className="font-mono text-sm"
              pattern="^#[0-9A-Fa-f]{6}$"
            />
          </div>
        </div>

        <div className="flex justify-end">
          <Button variant="outline" onClick={resetToDefault} className="text-sm">
            {t("profile.colors.reset")}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
