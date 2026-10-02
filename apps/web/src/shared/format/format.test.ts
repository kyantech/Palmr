import { describe, expect, test } from "vitest";
import { formatBytes, splitBytes, toBytes } from "./bytes";
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

describe("formatBytes", () => {
  test("scales to the largest whole binary unit and localizes the number", () => {
    expect(formatBytes(0, "en-US")).toBe("0 B");
    expect(formatBytes(1023, "en-US")).toBe("1,023 B");
    expect(formatBytes(1024, "en-US")).toBe("1 KiB");
    expect(formatBytes(1536, "en-US")).toBe("1.5 KiB");
    expect(formatBytes(5 * 1024 ** 3, "en-US")).toBe("5 GiB");
    expect(formatBytes(1536, "de-DE")).toBe("1,5 KiB");
  });
});

describe("splitBytes and toBytes", () => {
  test("round-trip a byte count through the largest exact unit", () => {
    expect(splitBytes(5 * 1024 ** 3)).toEqual({ amount: 5, unit: "GiB" });
    expect(splitBytes(3072)).toEqual({ amount: 3, unit: "KiB" });
    expect(splitBytes(1536)).toEqual({ amount: 1536, unit: "B" });
    expect(splitBytes(1001)).toEqual({ amount: 1001, unit: "B" });
    expect(splitBytes(0)).toEqual({ amount: 0, unit: "GiB" });
    expect(toBytes(1.5, "GiB")).toBe(1_610_612_736);
    expect(toBytes(0, "TiB")).toBe(0);
  });
});
