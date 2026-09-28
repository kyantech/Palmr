import { ConfigProvider } from "antd";
import { type ReactNode, useLayoutEffect, useMemo } from "react";
import type { TextDirection } from "../i18n/catalog";
import type { AntdLocale } from "../i18n/localeBridge";
import { type Appearance, composeTheme } from "./appearance";
import { useResolvedThemeMode } from "./useResolvedThemeMode";

interface ThemeProviderProps {
  appearance: Appearance;
  direction: TextDirection;
  antdLocale: AntdLocale;
  cspNonce: string | undefined;
  children: ReactNode;
}

export function ThemeProvider({
  appearance,
  direction,
  antdLocale,
  cspNonce,
  children,
}: ThemeProviderProps) {
  const mode = useResolvedThemeMode(appearance.preference);
  const { accent, primaryColor } = appearance;
  const themeConfig = useMemo(
    () => composeTheme(mode, { accent, primaryColor }),
    [mode, accent, primaryColor],
  );
  const csp = useMemo(
    () => (cspNonce === undefined ? {} : { csp: { nonce: cspNonce } }),
    [cspNonce],
  );

  useLayoutEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = mode;
    root.style.colorScheme = mode;
  }, [mode]);

  return (
    <ConfigProvider theme={themeConfig} direction={direction} locale={antdLocale} {...csp}>
      {children}
    </ConfigProvider>
  );
}
