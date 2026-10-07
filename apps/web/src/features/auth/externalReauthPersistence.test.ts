// @vitest-environment node
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";

const FILES = [
  "externalReauthChannel.ts",
  "externalReauthMessage.ts",
  "externalNavigation.ts",
  "store.ts",
  "components/useExternalReauth.ts",
  "components/RecentAuthModal.tsx",
  "routes/ReauthCompletePage.tsx",
  "api/mutations.ts",
];

const PERSISTENCE =
  /\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b|\bidb\b|document\.cookie|\bhistory\.(?:pushState|replaceState)\b|\bcaches\b|serviceWorker/;

const OPENER_DEPENDENCY =
  /\.opener\b|\bpostMessage\(|\.closed\b|event\.source|window\.addEventListener/;

function read(file: string) {
  return readFileSync(join(import.meta.dirname, file), "utf8");
}

describe("unit_external_recent_auth_has_no_persistence", () => {
  test.each(FILES)("%s never touches a browser persistence API", (file) => {
    expect(read(file)).not.toMatch(PERSISTENCE);
  });
});

describe("unit_external_recent_auth_has_no_opener_dependency", () => {
  test.each(FILES.filter((file) => file !== "externalReauthChannel.ts"))(
    "%s neither reads window.opener, popup.closed nor a window message",
    (file) => {
      const source = read(file).replace(/\btarget\.postMessage\(/g, "");

      expect(source).not.toMatch(OPENER_DEPENDENCY);
    },
  );

  test("the completion page talks only on its own challenge-specific channel", () => {
    const page = read("routes/ReauthCompletePage.tsx");

    expect(page).toContain("openExternalReauthChannel(channel)");
    expect(page).not.toMatch(/window\.postMessage|\.opener|\bopener\b/);
  });

  test("the parent listens only on its own challenge-specific channel", () => {
    const hook = read("components/useExternalReauth.ts");

    expect(hook).toContain("openExternalReauthChannel(channel)");
    expect(hook).not.toMatch(/window\.addEventListener|event\.source|event\.origin|\.opener\b/);
  });

  test("BroadcastChannel is constructed in exactly one place and always with the challenge id", () => {
    const helper = read("externalReauthChannel.ts");

    expect(helper.match(/new BroadcastChannel\(/g)).toHaveLength(1);
    expect(helper).toContain("new BroadcastChannel(externalReauthChannelName(channelId))");
    for (const file of FILES.filter((entry) => entry !== "externalReauthChannel.ts")) {
      expect(read(file)).not.toMatch(/new BroadcastChannel\(/);
    }
  });
});
