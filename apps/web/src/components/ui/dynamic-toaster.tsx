"use client";

import { useTheme } from "next-themes";
import { Toaster } from "sonner";

export function DynamicToaster() {
  const { theme, resolvedTheme } = useTheme();

  const currentTheme = resolvedTheme || theme;

  return (
    <Toaster
      position="top-right"
      expand={false}
      richColors={theme === "dark" ? true : false}
      closeButton={false}
      theme={currentTheme as "light" | "dark"}
    />
  );
}
