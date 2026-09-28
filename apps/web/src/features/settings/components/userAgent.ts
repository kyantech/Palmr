export interface UserAgentSummary {
  browser: string | null;
  system: string | null;
}

const BROWSERS: readonly (readonly [RegExp, string])[] = [
  [/\bEdg(?:e|A|iOS)?\//, "Edge"],
  [/\bOPR\/|\bOpera\b/, "Opera"],
  [/\bFirefox\/|\bFxiOS\//, "Firefox"],
  [/\b(?:Headless)?Chrome\/|\bCriOS\//, "Chrome"],
  [/\bSafari\//, "Safari"],
];

const SYSTEMS: readonly (readonly [RegExp, string])[] = [
  [/\biPhone|\biPad|\biPod/, "iOS"],
  [/\bAndroid\b/, "Android"],
  [/\bWindows\b/, "Windows"],
  [/\bCrOS\b/, "ChromeOS"],
  [/\bMac OS X\b|\bMacintosh\b/, "macOS"],
  [/\bLinux\b/, "Linux"],
];

function firstMatch(value: string, patterns: readonly (readonly [RegExp, string])[]) {
  return patterns.find(([pattern]) => pattern.test(value))?.[1] ?? null;
}

export function summarizeUserAgent(userAgent: string | null): UserAgentSummary {
  if (userAgent === null || userAgent.trim().length === 0) {
    return { browser: null, system: null };
  }
  return { browser: firstMatch(userAgent, BROWSERS), system: firstMatch(userAgent, SYSTEMS) };
}
