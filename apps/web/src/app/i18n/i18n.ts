import i18next, { type BackendModule, type i18n as I18n, type ResourceKey } from "i18next";
import { EAGER_NAMESPACES, FALLBACK_LOCALE, type LocaleCode, SUPPORTED_LOCALES } from "./catalog";

export type NamespaceLoader = (locale: string, namespace: string) => Promise<ResourceKey>;

const NAMESPACE_MODULES = import.meta.glob<ResourceKey>("./locales/*/*.json", {
  import: "default",
});

export const loadNamespace: NamespaceLoader = async (locale, namespace) => {
  const load = NAMESPACE_MODULES[`./locales/${locale}/${namespace}.json`];
  if (!load) {
    throw new Error(`No "${namespace}" namespace for locale ${locale}`);
  }
  return load();
};

function namespaceBackend(load: NamespaceLoader): BackendModule {
  return {
    type: "backend",
    init: () => undefined,
    read: (language, namespace, callback) => {
      load(language, namespace).then(
        (resources) => {
          callback(null, resources);
        },
        (error: unknown) => {
          callback(error instanceof Error ? error : new Error(String(error)), false);
        },
      );
    },
  };
}

export function createI18n(load: NamespaceLoader = loadNamespace): I18n {
  const instance = i18next.createInstance();
  void instance.use(namespaceBackend(load)).init({
    initAsync: false,
    fallbackLng: [FALLBACK_LOCALE],
    supportedLngs: [...SUPPORTED_LOCALES],
    nonExplicitSupportedLngs: false,
    load: "currentOnly",
    ns: [],
    defaultNS: "common",
    fallbackNS: false,
    partialBundledLanguages: true,
    returnNull: false,
    interpolation: { escapeValue: false },
    react: { useSuspense: true },
  });
  return instance;
}

export async function preloadEagerNamespaces(
  instance: I18n,
  locale: LocaleCode,
  load: NamespaceLoader = loadNamespace,
): Promise<void> {
  const locales = locale === FALLBACK_LOCALE ? [locale] : [locale, FALLBACK_LOCALE];
  const pending = locales.flatMap((code) =>
    EAGER_NAMESPACES.filter((namespace) => !instance.hasResourceBundle(code, namespace)).map(
      async (namespace) => {
        instance.addResourceBundle(code, namespace, await load(code, namespace), true, true);
      },
    ),
  );
  await Promise.all(pending);
}
