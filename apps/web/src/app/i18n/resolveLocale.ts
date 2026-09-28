import type { components } from "../../shared/api/schema";
import { FALLBACK_LOCALE, type LocaleCode, SUPPORTED_LOCALES } from "./catalog";

export interface LocaleInputs {
  authenticatedLocale?: components["schemas"]["MeUser"]["locale"] | null;
  visitorLocale?: string | null;
  browserLanguages?: readonly string[];
  instanceDefault?: components["schemas"]["Bootstrap"]["defaultLocale"] | null;
}

const BY_LOWERCASE = new Map<string, LocaleCode>(
  SUPPORTED_LOCALES.map((locale) => [locale.toLowerCase(), locale]),
);

const BY_LANGUAGE = new Map<string, LocaleCode>(
  SUPPORTED_LOCALES.map((locale) => [primaryLanguage(locale), locale]),
);

const TRADITIONAL_CHINESE = /^zh-(?:hant|tw|hk|mo)(?:-|$)/;

function primaryLanguage(tag: string): string {
  return tag.toLowerCase().split("-", 1)[0] ?? "";
}

function exactLocale(candidate: string | null | undefined): LocaleCode | undefined {
  return candidate
    ? BY_LOWERCASE.get(candidate.trim().replaceAll("_", "-").toLowerCase())
    : undefined;
}

export function matchBrowserLocale(languages: readonly string[]): LocaleCode | undefined {
  for (const language of languages) {
    const tag = language.trim().replaceAll("_", "-").toLowerCase();
    const exact = BY_LOWERCASE.get(tag);
    if (exact) {
      return exact;
    }
    if (tag !== "" && !TRADITIONAL_CHINESE.test(tag)) {
      const sameLanguage = BY_LANGUAGE.get(primaryLanguage(tag));
      if (sameLanguage) {
        return sameLanguage;
      }
    }
  }
  return undefined;
}

export function browserLanguages(
  navigatorLike: Pick<Navigator, "language" | "languages"> = navigator,
): readonly string[] {
  if (navigatorLike.languages.length > 0) {
    return navigatorLike.languages;
  }
  return navigatorLike.language ? [navigatorLike.language] : [];
}

export function resolveLocale({
  authenticatedLocale,
  visitorLocale,
  browserLanguages: languages = [],
  instanceDefault,
}: LocaleInputs): LocaleCode {
  return (
    exactLocale(authenticatedLocale) ??
    exactLocale(visitorLocale) ??
    matchBrowserLocale(languages) ??
    exactLocale(instanceDefault) ??
    FALLBACK_LOCALE
  );
}
