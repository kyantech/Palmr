import type { ConfigProviderProps } from "antd";
import dayjs from "dayjs";
import type { LocaleCode } from "./catalog";

export type AntdLocale = NonNullable<ConfigProviderProps["locale"]>;

type AntdLocaleModule = Promise<{ default: AntdLocale | { default: AntdLocale } }>;

const ANTD_LOCALES: Readonly<Record<LocaleCode, () => AntdLocaleModule>> = {
  "ar-SA": () => import("antd/locale/ar_EG"),
  "de-DE": () => import("antd/locale/de_DE"),
  "el-GR": () => import("antd/locale/el_GR"),
  "en-US": () => import("antd/locale/en_US"),
  "es-ES": () => import("antd/locale/es_ES"),
  "fa-IR": () => import("antd/locale/fa_IR"),
  "fr-FR": () => import("antd/locale/fr_FR"),
  "he-IL": () => import("antd/locale/he_IL"),
  "hi-IN": () => import("antd/locale/hi_IN"),
  "id-ID": () => import("antd/locale/id_ID"),
  "it-IT": () => import("antd/locale/it_IT"),
  "ja-JP": () => import("antd/locale/ja_JP"),
  "ko-KR": () => import("antd/locale/ko_KR"),
  "nl-NL": () => import("antd/locale/nl_NL"),
  "pl-PL": () => import("antd/locale/pl_PL"),
  "pt-BR": () => import("antd/locale/pt_BR"),
  "ru-RU": () => import("antd/locale/ru_RU"),
  "sv-SE": () => import("antd/locale/sv_SE"),
  "th-TH": () => import("antd/locale/th_TH"),
  "tr-TR": () => import("antd/locale/tr_TR"),
  "uk-UA": () => import("antd/locale/uk_UA"),
  "vi-VN": () => import("antd/locale/vi_VN"),
  "zh-CN": () => import("antd/locale/zh_CN"),
};

interface DayjsLocale {
  name: string;
  load: () => Promise<unknown>;
}

export const DAYJS_LOCALES: Readonly<Record<LocaleCode, DayjsLocale>> = {
  "ar-SA": { name: "ar-sa", load: () => import("dayjs/locale/ar-sa") },
  "de-DE": { name: "de", load: () => import("dayjs/locale/de") },
  "el-GR": { name: "el", load: () => import("dayjs/locale/el") },
  "en-US": { name: "en", load: () => import("dayjs/locale/en") },
  "es-ES": { name: "es", load: () => import("dayjs/locale/es") },
  "fa-IR": { name: "fa", load: () => import("dayjs/locale/fa") },
  "fr-FR": { name: "fr", load: () => import("dayjs/locale/fr") },
  "he-IL": { name: "he", load: () => import("dayjs/locale/he") },
  "hi-IN": { name: "hi", load: () => import("dayjs/locale/hi") },
  "id-ID": { name: "id", load: () => import("dayjs/locale/id") },
  "it-IT": { name: "it", load: () => import("dayjs/locale/it") },
  "ja-JP": { name: "ja", load: () => import("dayjs/locale/ja") },
  "ko-KR": { name: "ko", load: () => import("dayjs/locale/ko") },
  "nl-NL": { name: "nl", load: () => import("dayjs/locale/nl") },
  "pl-PL": { name: "pl", load: () => import("dayjs/locale/pl") },
  "pt-BR": { name: "pt-br", load: () => import("dayjs/locale/pt-br") },
  "ru-RU": { name: "ru", load: () => import("dayjs/locale/ru") },
  "sv-SE": { name: "sv", load: () => import("dayjs/locale/sv") },
  "th-TH": { name: "th", load: () => import("dayjs/locale/th") },
  "tr-TR": { name: "tr", load: () => import("dayjs/locale/tr") },
  "uk-UA": { name: "uk", load: () => import("dayjs/locale/uk") },
  "vi-VN": { name: "vi", load: () => import("dayjs/locale/vi") },
  "zh-CN": { name: "zh-cn", load: () => import("dayjs/locale/zh-cn") },
};

export async function loadAntdLocale(locale: LocaleCode): Promise<AntdLocale> {
  const { default: exported } = await ANTD_LOCALES[locale]();
  return "locale" in exported ? exported : exported.default;
}

export async function loadDayjsLocale(locale: LocaleCode): Promise<string> {
  const { name, load } = DAYJS_LOCALES[locale];
  await load();
  return name;
}

export function applyDayjsLocale(name: string): void {
  if (dayjs.locale() !== name) {
    dayjs.locale(name);
  }
}
