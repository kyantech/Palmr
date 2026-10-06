// @vitest-environment node
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";

const FILES = [
  "externalReauthMessage.ts",
  "externalNavigation.ts",
  "store.ts",
  "components/useExternalReauth.ts",
  "components/RecentAuthModal.tsx",
  "routes/ReauthCompletePage.tsx",
  "api/mutations.ts",
];

const PERSISTENCE =
  /\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b|\bidb\b|document\.cookie|\bhistory\.(?:pushState|replaceState)\b|\bcaches\b|BroadcastChannel|serviceWorker/;

describe("unit_external_recent_auth_has_no_persistence", () => {
  test.each(FILES)("%s never touches a browser persistence or side-channel API", (file) => {
    const source = readFileSync(join(import.meta.dirname, file), "utf8");

    expect(source).not.toMatch(PERSISTENCE);
  });

  test("the completion message is only ever posted to the exact origin", () => {
    const page = readFileSync(join(import.meta.dirname, "routes/ReauthCompletePage.tsx"), "utf8");

    expect(page).toContain("window.location.origin");
    expect(page).not.toMatch(/postMessage\([^)]*["']\*["']/);
  });

  test("the parent validates origin and the exact popup source before reading the message", () => {
    const hook = readFileSync(join(import.meta.dirname, "components/useExternalReauth.ts"), "utf8");

    expect(hook).toMatch(/event\.origin !== window\.location\.origin/);
    expect(hook).toMatch(/event\.source !== popup/);
  });
});
