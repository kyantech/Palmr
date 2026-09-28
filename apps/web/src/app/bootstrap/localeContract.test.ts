import { describe, expect, test, vi } from "vitest";
import { SUPPORTED_LOCALES } from "../i18n/catalog";
import {
  assertSupportedLocales,
  LocaleContractError,
  supportedLocalesMismatch,
} from "./localeContract";

describe("unit_supported_locales_contract", () => {
  test("the 23-locale catalogue is accepted in any order", () => {
    expect(supportedLocalesMismatch([...SUPPORTED_LOCALES].reverse())).toBeNull();
    expect(() => {
      assertSupportedLocales(SUPPORTED_LOCALES, true);
    }).not.toThrow();
  });

  test.each([
    ["a missing locale", SUPPORTED_LOCALES.filter((locale) => locale !== "pt-BR")],
    ["a 24th locale", [...SUPPORTED_LOCALES, "xx-XX"]],
    ["a duplicated locale", [...SUPPORTED_LOCALES, "en-US"]],
  ])("%s is a contract violation", (_label, list) => {
    expect(supportedLocalesMismatch(list)).not.toBeNull();
    expect(() => {
      assertSupportedLocales(list, true);
    }).toThrow(LocaleContractError);
  });

  test("outside development a violation is reported, never silently accepted", () => {
    const report = vi.spyOn(console, "error").mockImplementation(() => undefined);

    assertSupportedLocales([...SUPPORTED_LOCALES, "xx-XX"], false);

    expect(report).toHaveBeenCalledWith(expect.stringContaining("xx-XX"));
    report.mockRestore();
  });
});
