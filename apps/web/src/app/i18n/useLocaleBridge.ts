import type { i18n as I18n } from "i18next";
import { useEffect, useLayoutEffect, useState } from "react";
import { type LocaleCode, localeDirection, type TextDirection } from "./catalog";
import { loadNamespace, type NamespaceLoader, preloadEagerNamespaces } from "./i18n";
import { type AntdLocale, applyDayjsLocale, loadAntdLocale, loadDayjsLocale } from "./localeBridge";

export interface LocaleLoaders {
  antd: (locale: LocaleCode) => Promise<AntdLocale>;
  dayjs: (locale: LocaleCode) => Promise<string>;
  namespace: NamespaceLoader;
}

export interface ActiveLocale {
  locale: LocaleCode;
  direction: TextDirection;
  antdLocale: AntdLocale;
}

export const DEFAULT_LOCALE_LOADERS: LocaleLoaders = {
  antd: loadAntdLocale,
  dayjs: loadDayjsLocale,
  namespace: loadNamespace,
};

export function useLocaleBridge(
  i18n: I18n,
  locale: LocaleCode,
  loaders: LocaleLoaders = DEFAULT_LOCALE_LOADERS,
): ActiveLocale | null {
  const [active, setActive] = useState<ActiveLocale | null>(null);
  const [failure, setFailure] = useState<{ error: unknown } | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    const superseded = () => controller.signal.aborted;
    const activate = async () => {
      const [antdLocale, dayjsLocale] = await Promise.all([
        loaders.antd(locale),
        loaders.dayjs(locale),
        preloadEagerNamespaces(i18n, locale, loaders.namespace),
      ]);
      if (superseded()) {
        return;
      }
      await i18n.changeLanguage(locale);
      if (superseded()) {
        return;
      }
      applyDayjsLocale(dayjsLocale);
      setActive({ locale, direction: localeDirection(locale), antdLocale });
    };
    activate().catch((error: unknown) => {
      if (!superseded()) {
        setFailure({ error });
      }
    });
    return () => {
      controller.abort();
    };
  }, [i18n, locale, loaders]);

  useLayoutEffect(() => {
    if (active) {
      document.documentElement.lang = active.locale;
      document.documentElement.dir = active.direction;
    }
  }, [active]);

  if (failure) {
    throw failure.error;
  }
  return active;
}
