"use client";

import { useEffect } from "react";

const COLOR_STORAGE_KEY = "palmr-custom-primary-color";
const FONT_STORAGE_KEY = "palmr-custom-font-family";
const RADIUS_STORAGE_KEY = "palmr-custom-radius";
const BACKGROUND_STORAGE_KEY = "palmr-custom-background";

export function ThemeColorProvider({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    const savedColor = localStorage.getItem(COLOR_STORAGE_KEY);
    if (savedColor) {
      document.documentElement.style.setProperty("--primary", savedColor);
      document.documentElement.style.setProperty("--sidebar-primary", savedColor);
      document.documentElement.style.setProperty("--ring", savedColor);
      document.documentElement.style.setProperty("--sidebar-ring", savedColor);
    }

    const savedFont = localStorage.getItem(FONT_STORAGE_KEY);
    if (savedFont) {
      document.documentElement.style.setProperty("--custom-font-family", savedFont);
      document.documentElement.style.setProperty("--font-sans", savedFont);
      document.documentElement.style.setProperty("--font-serif", savedFont);
      document.body.style.fontFamily = savedFont;
    }

    const savedRadius = localStorage.getItem(RADIUS_STORAGE_KEY);
    if (savedRadius) {
      document.documentElement.style.setProperty("--radius", savedRadius);
    }

    const savedBackground = localStorage.getItem(BACKGROUND_STORAGE_KEY);
    if (savedBackground) {
      const parsed = JSON.parse(savedBackground);
      document.documentElement.style.setProperty("--custom-background-light", parsed.light);
      document.documentElement.style.setProperty("--custom-background-dark", parsed.dark);
    }
  }, []);

  return <>{children}</>;
}
