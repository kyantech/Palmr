import { cookies } from "next/headers";
import { getRequestConfig } from "next-intl/server";

const supportedLocales = [
  "en-US",
  "pt-BR",
  "fr-FR",
  "es-ES",
  "de-DE",
  "it-IT",
  "nl-NL",
  "pl-PL",
  "tr-TR",
  "ru-RU",
  "hi-IN",
  "ar-SA",
  "zh-CN",
  "ja-JP",
  "ko-KR",
];

const envDefault = process.env.NEXT_PUBLIC_DEFAULT_LANGUAGE || "en-US";
const DEFAULT_LOCALE = supportedLocales.includes(envDefault) ? envDefault : "en-US";

export default getRequestConfig(async ({ locale }) => {
  const cookieStore = cookies();
  const cookiesList = await cookieStore;
  const localeCookie = cookiesList.get("NEXT_LOCALE");

  const resolvedLocale = localeCookie?.value || locale || DEFAULT_LOCALE;
  const finalLocale = supportedLocales.includes(resolvedLocale) ? resolvedLocale : DEFAULT_LOCALE;

  try {
    return {
      locale: finalLocale,
      messages: (await import(`../../messages/${finalLocale}.json`)).default,
    };
  } catch {
    return {
      locale: DEFAULT_LOCALE,
      messages: (await import(`../../messages/${DEFAULT_LOCALE}.json`)).default,
    };
  }
});
