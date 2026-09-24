import { expect, test } from "vitest";
import { CSRF_COOKIE, CSRF_HEADER, readCookie } from "./csrf";

test("the CSRF cookie and header names", () => {
  expect(CSRF_COOKIE).toBe("palmr_csrf");
  expect(CSRF_HEADER).toBe("X-Palmr-CSRF");
});

test.each([
  ["palmr_csrf2=wrong; palmr_csrf=correct", "correct"],
  ["palmr_csrf=correct; palmr_csrf2=wrong", "correct"],
  ["xpalmr_csrf=wrong;palmr_csrf=correct", "correct"],
  ["palmr_csrf=a=b", "a=b"],
  ["palmr_csrf=", ""],
  ["palmr_csrf2=wrong; xpalmr_csrf=wrong", null],
  ["", null],
])("readCookie in %j", (cookies, expected) => {
  expect(readCookie(CSRF_COOKIE, cookies)).toBe(expected);
});

test("cookie names are matched literally, not as patterns", () => {
  expect(readCookie("palmr.csrf", "palmr_csrf=x; palmrXcsrf=y")).toBeNull();
  expect(readCookie(".*", "palmr_csrf=x")).toBeNull();
});
