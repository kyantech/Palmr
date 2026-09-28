import { describe, expect, test } from "vitest";
import { summarizeUserAgent } from "./userAgent";

describe("summarizeUserAgent", () => {
  test.each([
    [
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
      { browser: "Chrome", system: "macOS" },
    ],
    [
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0",
      { browser: "Edge", system: "Windows" },
    ],
    [
      "Mozilla/5.0 (X11; Linux x86_64; rv:143.0) Gecko/20100101 Firefox/143.0",
      { browser: "Firefox", system: "Linux" },
    ],
    [
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
      { browser: "Safari", system: "iOS" },
    ],
    [
      "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36",
      { browser: "Chrome", system: "Android" },
    ],
    [
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/140.0.0.0 Safari/537.36",
      { browser: "Chrome", system: "macOS" },
    ],
    ["curl/8.9.1", { browser: null, system: null }],
  ])("%s", (userAgent, expected) => {
    expect(summarizeUserAgent(userAgent)).toEqual(expected);
  });

  test("a missing user agent is reported as unknown rather than guessed", () => {
    expect(summarizeUserAgent(null)).toEqual({ browser: null, system: null });
    expect(summarizeUserAgent("   ")).toEqual({ browser: null, system: null });
  });
});
