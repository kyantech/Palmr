import { describe, expect, test } from "vitest";
import type { Bootstrap } from "./queries";

// `/auth/me` is the only session authority. The bootstrap payload is flat
// public instance/login configuration and must never grow a session member.
type SessionMembers = Extract<keyof Bootstrap, "session" | "user" | "restriction">;
type BootstrapHasNoSession = SessionMembers extends never ? true : false;

describe("unit_bootstrap_has_no_session_state", () => {
  test("the Bootstrap wire type declares no session, user or restriction member", () => {
    const noSession: BootstrapHasNoSession = true;
    expect(noSession).toBe(true);
  });

  test("the public instance fields the app reads are present", () => {
    const fields = [
      "setupCompleted",
      "appName",
      "appDescription",
      "defaultLocale",
      "supportedLocales",
      "passwordLoginEnabled",
      "providers",
      "poweredByVisible",
      "version",
    ] satisfies readonly (keyof Bootstrap)[];
    expect(fields).toHaveLength(9);
  });
});
