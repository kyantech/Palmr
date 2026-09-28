import { useCallback, useSyncExternalStore } from "react";
import type { ResolvedThemeMode, ThemePreference } from "./appearance";

export const DARK_SCHEME_QUERY = "(prefers-color-scheme: dark)";

function darkSchemeQuery(): MediaQueryList | null {
  return typeof window.matchMedia === "function" ? window.matchMedia(DARK_SCHEME_QUERY) : null;
}

function explicitMode(preference: ThemePreference): ResolvedThemeMode {
  return preference === "dark" ? "dark" : "light";
}

export function useResolvedThemeMode(preference: ThemePreference): ResolvedThemeMode {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const query = preference === "system" ? darkSchemeQuery() : null;
      if (!query) {
        return () => undefined;
      }
      query.addEventListener("change", onChange);
      return () => {
        query.removeEventListener("change", onChange);
      };
    },
    [preference],
  );

  const getSnapshot = (): ResolvedThemeMode => {
    if (preference !== "system") {
      return preference;
    }
    return darkSchemeQuery()?.matches ? "dark" : "light";
  };

  return useSyncExternalStore(subscribe, getSnapshot, () => explicitMode(preference));
}
