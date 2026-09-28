import { describe, expect, test } from "vitest";
import { loginPathWithNext, safeNextPath } from "./next";

describe("unit_next_param_validation", () => {
  test.each([
    ["/files", "/files"],
    ["/files/abc?sort=name", "/files/abc?sort=name"],
    ["/settings/security", "/settings/security"],
    ["/files/abc?sort=name&dir=desc#row-3", "/files/abc?sort=name&dir=desc#row-3"],
    ["/overview", "/overview"],
  ])("accepts the internal path %s", (value, expected) => {
    expect(safeNextPath(value)).toBe(expected);
  });

  test.each([
    ["empty value", ""],
    ["null", null],
    ["undefined", undefined],
    ["protocol-relative //", "//evil.example"],
    ["protocol-relative with path", "//evil.example/files"],
    ["bare //", "//"],
    ["absolute https URL", "https://evil.example"],
    ["absolute http URL", "http://evil.example/files"],
    ["javascript: URL", "javascript:alert(1)"],
    ["data: URL", "data:text/html,hi"],
    ["backslash trick", "/\\evil.example"],
    ["double backslash", "\\\\evil.example"],
    ["mixed slash/backslash", "\\/evil.example"],
    ["tab-smuggled protocol-relative", "/\t/evil.example"],
    ["newline-smuggled protocol-relative", "/\n/evil.example"],
    ["leading whitespace", " /files"],
    ["non-leading-slash relative", "files"],
    ["dot-relative", "./files"],
    ["dot-segment collapsing into //", "/a/..//evil.example"],
  ])("rejects %s", (_label, value) => {
    expect(safeNextPath(value)).toBeNull();
  });

  test("invalid values fall back to plain /login when building the login redirect", () => {
    expect(loginPathWithNext("//evil.example", "")).toBe("/login");
    expect(loginPathWithNext("/", "")).toBe("/login");
  });

  test("valid values are encoded into ?next= as an application-relative path", () => {
    expect(loginPathWithNext("/files/abc", "?sort=name")).toBe(
      `/login?next=${encodeURIComponent("/files/abc?sort=name")}`,
    );
  });
});
