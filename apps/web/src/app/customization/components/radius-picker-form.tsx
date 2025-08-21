"use client";

import { useCallback, useEffect, useState } from "react";
import { IconBorderRadius, IconChevronDown, IconChevronUp } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";

const PREDEFINED_RADIUS = [
  { name: "None", value: "0rem", description: "Sharp corners" },
  { name: "Small", value: "0.25rem", description: "Slightly rounded" },
  { name: "Medium", value: "0.5rem", description: "Balanced rounding" },
  { name: "Large", value: "0.75rem", description: "More rounded" },
  { name: "Extra Large", value: "1rem", description: "Very rounded" },
  { name: "Maximum", value: "1.5rem", description: "Fully rounded" },
];

const STORAGE_KEY = "palmr-custom-radius";

export function RadiusPickerForm() {
  const t = useTranslations();
  const [selectedRadius, setSelectedRadius] = useState(PREDEFINED_RADIUS[2].value);
  const [isCollapsed, setIsCollapsed] = useState(true);
  const applyRadius = useCallback((radiusValue: string) => {
    document.documentElement.style.setProperty("--radius", radiusValue);
    console.log("Applied radius:", radiusValue);
  }, []);

  useEffect(() => {
    const savedRadius = localStorage.getItem(STORAGE_KEY);
    if (savedRadius) {
      setSelectedRadius(savedRadius);
      applyRadius(savedRadius);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleRadiusSelect = (radiusValue: string) => {
    setSelectedRadius(radiusValue);
    applyRadius(radiusValue);
    localStorage.setItem(STORAGE_KEY, radiusValue);
  };

  const resetToDefault = () => {
    const defaultRadius = PREDEFINED_RADIUS[2].value;
    handleRadiusSelect(defaultRadius);
    localStorage.removeItem(STORAGE_KEY);
  };

  return (
    <Card className="p-6 gap-0">
      <CardHeader
        className="flex flex-row items-center justify-between cursor-pointer p-0"
        onClick={() => setIsCollapsed(!isCollapsed)}
      >
        <div className="flex flex-row items-center gap-8">
          <IconBorderRadius className="text-xl text-muted-foreground" />
          <div className="flex flex-col gap-1">
            <h2 className="text-xl font-semibold">{t("customization.radius.title")}</h2>
            <p className="text-sm text-muted-foreground">{t("customization.radius.description")}</p>
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
            <Label className="text-sm font-medium mb-6">{t("customization.radius.available")}</Label>
            <div className="grid grid-cols-2 lg:grid-cols-3 gap-4">
              {PREDEFINED_RADIUS.map((radius) => (
                <button
                  key={radius.name}
                  onClick={() => handleRadiusSelect(radius.value)}
                  className={`relative p-4 rounded-xl border-2 transition-all hover:shadow-md text-center group ${
                    selectedRadius === radius.value
                      ? "border-primary ring-2 ring-primary ring-offset-2 bg-primary/5"
                      : "border-gray-200 dark:border-gray-600 hover:border-gray-300 dark:hover:border-gray-500 hover:bg-muted/30"
                  }`}
                  type="button"
                >
                  <div className="flex flex-col items-center gap-3">
                    <div className="flex flex-col items-center gap-2">
                      <span className="font-medium text-base">{radius.name}</span>
                      <span className="text-xs text-muted-foreground">{radius.description}</span>
                    </div>
                    <div
                      className="w-12 h-8 bg-primary/20 border border-primary/30"
                      style={{ borderRadius: radius.value }}
                    />
                  </div>
                </button>
              ))}
            </div>
            <p className="text-xs text-muted-foreground ml-1 mt-6">{t("customization.radius.availableDescription")}</p>
          </div>
        </div>
        <div className="flex justify-between items-center mt-4">
          <div className="flex"></div>
          <div className="flex">
            <Button variant="outline" onClick={resetToDefault} className="text-sm">
              {t("customization.radius.reset")}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
