"use client";

import { useCallback, useEffect, useState } from "react";
import { IconChevronDown, IconChevronUp, IconTypography } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";

const PREDEFINED_FONTS = [
  { name: "Outfit", value: "var(--font-outfit), Outfit, sans-serif" },
  { name: "Inter", value: "var(--font-inter), Inter, sans-serif" },
  { name: "Roboto", value: "var(--font-roboto), Roboto, sans-serif" },
  { name: "Open Sans", value: "var(--font-open-sans), 'Open Sans', sans-serif" },
  { name: "Poppins", value: "var(--font-poppins), Poppins, sans-serif" },
  { name: "Nunito", value: "var(--font-nunito), Nunito, sans-serif" },
  { name: "Lato", value: "var(--font-lato), Lato, sans-serif" },
  { name: "Montserrat", value: "var(--font-montserrat), Montserrat, sans-serif" },
  { name: "Source Sans 3", value: "var(--font-source-sans), 'Source Sans 3', sans-serif" },
  { name: "Raleway", value: "var(--font-raleway), Raleway, sans-serif" },
  { name: "Work Sans", value: "var(--font-work-sans), 'Work Sans', sans-serif" },
];

const STORAGE_KEY = "palmr-custom-font-family";

export function FontPickerForm() {
  const t = useTranslations();
  const [selectedFont, setSelectedFont] = useState(PREDEFINED_FONTS[0].value);
  const [isCollapsed, setIsCollapsed] = useState(true);

  const applyFont = useCallback((fontValue: string) => {
    document.documentElement.style.setProperty("--custom-font-family", fontValue);
    document.documentElement.style.setProperty("--font-sans", fontValue);
    document.documentElement.style.setProperty("--font-serif", fontValue);

    document.body.style.fontFamily = fontValue;
  }, []);

  useEffect(() => {
    const savedFont = localStorage.getItem(STORAGE_KEY);
    if (savedFont) {
      setSelectedFont(savedFont);
      applyFont(savedFont);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleFontSelect = (fontValue: string) => {
    setSelectedFont(fontValue);
    applyFont(fontValue);
    localStorage.setItem(STORAGE_KEY, fontValue);
  };

  const resetToDefault = () => {
    const defaultFont = PREDEFINED_FONTS[0].value;
    handleFontSelect(defaultFont);
    localStorage.removeItem(STORAGE_KEY);
  };

  return (
    <Card className="p-6 gap-0">
      <CardHeader
        className="flex flex-row items-center justify-between cursor-pointer p-0"
        onClick={() => setIsCollapsed(!isCollapsed)}
      >
        <div className="flex flex-row items-center gap-8">
          <IconTypography className="text-xl text-muted-foreground" />
          <div className="flex flex-col gap-1">
            <h2 className="text-xl font-semibold">{t("customization.fonts.title")}</h2>
            <p className="text-sm text-muted-foreground">{t("customization.fonts.description")}</p>
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
            <Label className="text-sm font-medium mb-6">{t("customization.fonts.available")}</Label>
            <div className="grid grid-cols-2 lg:grid-cols-3 gap-4">
              {PREDEFINED_FONTS.map((font) => (
                <button
                  key={font.name}
                  onClick={() => handleFontSelect(font.value)}
                  className={`relative p-4 rounded-xl border-2 transition-all hover:shadow-md text-center group ${
                    selectedFont === font.value
                      ? "border-primary ring-2 ring-primary ring-offset-2 bg-primary/5"
                      : "border-gray-200 dark:border-gray-600 hover:border-gray-300 dark:hover:border-gray-500 hover:bg-muted/30"
                  }`}
                  type="button"
                >
                  <span
                    className="font-medium text-lg group-hover:text-primary transition-colors"
                    style={{ fontFamily: font.value }}
                  >
                    {font.name}
                  </span>
                </button>
              ))}
            </div>
            <p className="text-xs text-muted-foreground ml-1 mt-6">{t("customization.fonts.availableDescription")}</p>
          </div>
        </div>
        <div className="flex justify-between items-center mt-4">
          <div className="flex"></div>
          <div className="flex">
            <Button variant="outline" onClick={resetToDefault} className="text-sm">
              {t("customization.fonts.reset")}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
