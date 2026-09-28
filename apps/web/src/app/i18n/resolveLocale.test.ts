import { describe, expect, expectTypeOf, test } from "vitest";
import {
  FALLBACK_LOCALE,
  isLocaleCode,
  type LocaleCode,
  localeDirection,
  RTL_LOCALES,
  SUPPORTED_LOCALES,
} from "./catalog";
import { browserLanguages, matchBrowserLocale, resolveLocale } from "./resolveLocale";

test("unit_resolve_locale_precedence", () => {
  const everything = {
    authenticatedLocale: "de-DE",
    visitorLocale: "fr-FR",
    browserLanguages: ["ja-JP"],
    instanceDefault: "pt-BR",
  };

  expect(resolveLocale(everything)).toBe("de-DE");
  expect(resolveLocale({ ...everything, authenticatedLocale: null })).toBe("fr-FR");
  expect(resolveLocale({ ...everything, authenticatedLocale: null, visitorLocale: null })).toBe(
    "ja-JP",
  );
  expect(resolveLocale({ browserLanguages: [], instanceDefault: "pt-BR" })).toBe("pt-BR");
  expect(resolveLocale({})).toBe("en-US");

  expect(
    resolveLocale({
      authenticatedLocale: "xx-XX",
      visitorLocale: "en-XA",
      browserLanguages: ["tlh", "pt-PT"],
      instanceDefault: "de-DE",
    }),
  ).toBe("pt-BR");
  expect(
    resolveLocale({ browserLanguages: ["tlh", "zh-TW"], instanceDefault: "not-a-locale" }),
  ).toBe("en-US");

  const rtl = resolveLocale({ authenticatedLocale: "he-IL", browserLanguages: ["en-US"] });
  expect(rtl).toBe("he-IL");
  expect(localeDirection(rtl)).toBe("rtl");
  expect(localeDirection(resolveLocale({ browserLanguages: ["ar"] }))).toBe("rtl");
  expect(localeDirection(resolveLocale({ browserLanguages: ["fa-AF"] }))).toBe("rtl");
});

describe("browser locale matching", () => {
  test.each([
    [["pt"], "pt-BR"],
    [["pt-PT"], "pt-BR"],
    [["pt-BR"], "pt-BR"],
    [["pt-br"], "pt-BR"],
    [["en"], "en-US"],
    [["en-GB"], "en-US"],
    [["zh"], "zh-CN"],
    [["zh-CN"], "zh-CN"],
    [["zh-Hans-CN"], "zh-CN"],
    [["de_AT"], "de-DE"],
  ] as const)("%j matches %s", (languages, expected) => {
    expect(matchBrowserLocale(languages)).toBe(expected);
  });

  test("an earlier preference wins over a later exact match", () => {
    expect(matchBrowserLocale(["pt-PT", "en-US"])).toBe("pt-BR");
  });

  test("unsupported languages and Traditional Chinese are skipped", () => {
    expect(matchBrowserLocale(["tlh", "", "zh-TW", "zh-Hant", "zh-HK"])).toBeUndefined();
    expect(matchBrowserLocale(["zh-TW", "ja"])).toBe("ja-JP");
  });

  test("navigator.languages is preferred, navigator.language is the fallback", () => {
    expect(browserLanguages({ languages: ["fr-FR", "en"], language: "de-DE" })).toEqual([
      "fr-FR",
      "en",
    ]);
    expect(browserLanguages({ languages: [], language: "de-DE" })).toEqual(["de-DE"]);
    expect(browserLanguages({ languages: [], language: "" })).toEqual([]);
  });
});

describe("the locale catalogue", () => {
  test("exactly the 23 product locales are registered, en-US is the fallback", () => {
    expect(SUPPORTED_LOCALES).toHaveLength(23);
    expect(new Set(SUPPORTED_LOCALES).size).toBe(23);
    expect([...SUPPORTED_LOCALES].sort()).toEqual([
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
    ]);
    expect(FALLBACK_LOCALE).toBe("en-US");
  });

  test("the en-XA pseudolocale is not a selectable product locale", () => {
    expect(isLocaleCode("en-XA")).toBe(false);
    expect(resolveLocale({ authenticatedLocale: "en-XA", visitorLocale: "en-XA" })).toBe("en-US");
    expectTypeOf<"en-XA">().not.toExtend<LocaleCode>();
  });

  test("exactly ar-SA, fa-IR and he-IL are right-to-left", () => {
    expect([...RTL_LOCALES].sort()).toEqual(["ar-SA", "fa-IR", "he-IL"]);
    expect(localeDirection("ar-SA")).toBe("rtl");
    expect(localeDirection("fa-IR")).toBe("rtl");
    expect(localeDirection("he-IL")).toBe("rtl");
    expect(localeDirection("en-US")).toBe("ltr");
    expect(localeDirection("pt-BR")).toBe("ltr");
    expect(SUPPORTED_LOCALES.filter((locale) => localeDirection(locale) === "rtl")).toHaveLength(3);
  });
});
