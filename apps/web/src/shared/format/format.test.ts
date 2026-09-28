import { describe, expect, test } from "vitest";
import { formatDateTime } from "./dateTime";
import { localeDisplayName, localeOptions } from "./locale";

describe("localeOptions", () => {
  test("labels each code with its own endonym and sorts by label", () => {
    const options = localeOptions(["pt-BR", "de-DE", "en-US"]);
    expect(options.map((option) => option.value)).toEqual(["de-DE", "en-US", "pt-BR"]);
    expect(options[0]?.label).toBe(localeDisplayName("de-DE"));
    expect(localeDisplayName("de-DE")).toBe("Deutsch (Deutschland)");
  });

  test("an unknown tag falls back to the code itself", () => {
    expect(localeDisplayName("not a locale")).toBe("not a locale");
  });
});

describe("formatDateTime", () => {
  test("formats with the requested locale", () => {
    const value = "2026-09-28T12:34:00Z";
    expect(formatDateTime(value, "en-US")).toBe(
      new Intl.DateTimeFormat("en-US", { dateStyle: "medium", timeStyle: "short" }).format(
        new Date(value),
      ),
    );
    expect(formatDateTime(value, "de-DE")).not.toBe(formatDateTime(value, "en-US"));
  });

  test("an unparseable timestamp is returned as received", () => {
    expect(formatDateTime("not a date", "en-US")).toBe("not a date");
  });
});
