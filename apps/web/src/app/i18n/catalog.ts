export const SUPPORTED_LOCALES = [
  "ar-SA",
  "de-DE",
  "el-GR",
  "en-US",
  "es-ES",
  "fa-IR",
  "fr-FR",
  "he-IL",
  "hi-IN",
  "id-ID",
  "it-IT",
  "ja-JP",
  "ko-KR",
  "nl-NL",
  "pl-PL",
  "pt-BR",
  "ru-RU",
  "sv-SE",
  "th-TH",
  "tr-TR",
  "uk-UA",
  "vi-VN",
  "zh-CN",
] as const;

export type LocaleCode = (typeof SUPPORTED_LOCALES)[number];

export const FALLBACK_LOCALE = "en-US" satisfies LocaleCode;

export const RTL_LOCALES: ReadonlySet<LocaleCode> = new Set(["ar-SA", "fa-IR", "he-IL"]);

export const NAMESPACES = [
  "common",
  "auth",
  "setup",
  "overview",
  "files",
  "shares",
  "reverseShares",
  "received",
  "transfers",
  "settings",
  "admin",
  "public",
  "errors",
  "emails",
  "validation",
] as const;

export type Namespace = (typeof NAMESPACES)[number];

export const EAGER_NAMESPACES = ["errors", "common"] as const satisfies readonly Namespace[];

export type TextDirection = "ltr" | "rtl";

export function isLocaleCode(value: unknown): value is LocaleCode {
  return typeof value === "string" && (SUPPORTED_LOCALES as readonly string[]).includes(value);
}

export function localeDirection(locale: LocaleCode): TextDirection {
  return RTL_LOCALES.has(locale) ? "rtl" : "ltr";
}
