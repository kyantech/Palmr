import type { i18n as I18n } from "i18next";
import type { ReactNode } from "react";
import type { LocaleCode } from "./i18n/catalog";
import { I18nProvider } from "./i18n/I18nProvider";
import {
  DEFAULT_LOCALE_LOADERS,
  type LocaleLoaders,
  useLocaleBridge,
} from "./i18n/useLocaleBridge";
import type { Appearance } from "./theme/appearance";
import { ThemeProvider } from "./theme/ThemeProvider";

interface PresentationProvidersProps {
  i18n: I18n;
  locale: LocaleCode;
  appearance: Appearance;
  cspNonce: string | undefined;
  loaders?: LocaleLoaders;
  children: ReactNode;
}

export function PresentationProviders({
  i18n,
  locale,
  appearance,
  cspNonce,
  loaders = DEFAULT_LOCALE_LOADERS,
  children,
}: PresentationProvidersProps) {
  const active = useLocaleBridge(i18n, locale, loaders);
  if (!active) {
    return null;
  }
  return (
    <ThemeProvider
      appearance={appearance}
      direction={active.direction}
      antdLocale={active.antdLocale}
      cspNonce={cspNonce}
    >
      <I18nProvider i18n={i18n}>{children}</I18nProvider>
    </ThemeProvider>
  );
}
