import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { qk } from "../../shared/api/query-keys";
import { errorEnvelope } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { installSettingsServer, type SettingsServerOptions } from "../../test/settingsServer";
import { localeDisplayName } from "../../shared/format/locale";
import { SUPPORTED_LOCALES } from "../i18n/catalog";
import { appRoutes } from "./routes";

const CURATED_ACCENTS = ["Instance default", "Blue", "Violet", "Emerald", "Amber", "Rose", "Slate"];

function renderSettings(path: string, options: SettingsServerOptions = {}) {
  const state = installSettingsServer(options);
  const session = renderSession({ routes: appRoutes, initialEntries: [path] });
  return { state, ...session, user: userEvent.setup({ delay: null }) };
}

function radioGroup(name: string) {
  return screen.getByRole("radiogroup", { name });
}

function radioLabel(group: string, name: RegExp | string): HTMLElement {
  const label = within(radioGroup(group)).getByRole("radio", { name }).closest("label");
  if (label === null) {
    throw new Error(`radio ${String(name)} has no label`);
  }
  return label;
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
  delete document.documentElement.dataset.theme;
});

describe("settings routing", () => {
  test("/settings lands on the profile section inside the AppShell with local section navigation", async () => {
    const { router } = renderSettings("/settings");

    expect(await screen.findByRole("heading", { level: 1, name: "Settings" })).toBeDefined();
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/settings/profile");
    });
    expect(screen.getByTestId("app-shell")).toBeDefined();
    const sections = screen.getByRole("navigation", { name: "Settings sections" });
    expect(
      within(sections)
        .getAllByRole("link")
        .map((link) => [link.textContent, link.getAttribute("href")]),
    ).toEqual([
      ["Profile", "/settings/profile"],
      ["Appearance", "/settings/appearance"],
      ["Security", "/settings/security"],
      ["Sessions", "/settings/sessions"],
    ]);
    await waitFor(() => {
      expect(
        within(sections)
          .getAllByRole("link")
          .filter((link) => link.getAttribute("aria-current") === "page")
          .map((link) => link.textContent),
      ).toEqual(["Profile"]);
    });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  test("anonymous visitors are sent to /login by the guards with the settings path preserved", async () => {
    const state = installSettingsServer();
    state.me = null;
    const { router } = renderSession({ routes: appRoutes, initialEntries: ["/settings/sessions"] });

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
    expect(router.state.location.search).toBe("?next=%2Fsettings%2Fsessions");
    expect(state.calls.sessions).toBe(0);
  });
});

describe("profile reconciliation", () => {
  test("a saved name refreshes /auth/me so the shell identity shows the server's value", async () => {
    const { state, user, queryClient } = renderSettings("/settings/profile");
    const firstName = await screen.findByLabelText("First name");
    const account = screen.getByRole("button", { name: "Account menu" });
    expect(within(account).getByText("Ada Lovelace")).toBeDefined();
    const meCalls = state.calls.me;

    await user.clear(firstName);
    await user.type(firstName, "Grace");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(within(account).getByText("Grace Lovelace")).toBeDefined();
    });
    expect(state.profileBodies).toEqual([{ firstName: "Grace", lastName: "Lovelace" }]);
    expect(state.calls.me).toBeGreaterThan(meCalls);
    expect(queryClient.getQueryData(qk.me.profile())).toMatchObject({ firstName: "Grace" });
  });
});

describe("appearance preferences", () => {
  test("offers exactly Light/Dark/System, the curated accents and the supported locales", async () => {
    renderSettings("/settings/appearance");
    await screen.findByRole("heading", { level: 2, name: "Appearance" });

    expect(
      within(radioGroup("Theme"))
        .getAllByRole("radio")
        .map((radio) => radio.closest("label")?.textContent),
    ).toEqual(["Light", "Dark", "System"]);
    expect(
      within(radioGroup("Accent color"))
        .getAllByRole("radio")
        .map((radio) => radio.closest("label")?.textContent),
    ).toEqual(CURATED_ACCENTS);
    expect(screen.queryByRole("textbox", { name: /color|hex|css|font|radius/i })).toBeNull();
    expect(document.querySelector("input[type='color']")).toBeNull();

    await userEvent.click(screen.getByRole("combobox"));
    await screen.findAllByTitle(localeDisplayName("de-DE"));
    for (const code of SUPPORTED_LOCALES) {
      expect(screen.getAllByTitle(localeDisplayName(code)).length).toBeGreaterThan(0);
    }
  });

  test("a theme change applies immediately, persists and is reconciled with /auth/me", async () => {
    const { state, user, queryClient } = renderSettings("/settings/appearance", {
      latencyMs: 150,
    });
    await screen.findByRole("heading", { level: 2, name: "Appearance" });
    expect(document.documentElement.dataset.theme).toBe("light");

    await user.click(radioLabel("Theme", "Dark"));

    expect(await screen.findByText("Saving…")).toBeDefined();
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(await screen.findByText("Saved")).toBeDefined();
    expect(state.preferenceBodies).toEqual([{ theme: "dark" }]);
    await waitFor(() => {
      expect(queryClient.getQueryData(qk.me.current())).toMatchObject({ user: { theme: "dark" } });
    });
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  test("a failed save rolls the visible preference back to the authoritative value", async () => {
    const { state, user, queryClient } = renderSettings("/settings/appearance", {
      preferencesFailure: () =>
        errorEnvelope("INTERNAL_ERROR", 500, "req-pref-failed", { message: "disk full" }),
    });
    await screen.findByRole("heading", { level: 2, name: "Appearance" });

    await user.click(radioLabel("Accent color", /Rose/));

    const alert = await screen.findByRole("alert");
    expect(alert.querySelector("[data-request-id='req-pref-failed']")).not.toBeNull();
    expect(state.preferenceBodies).toEqual([{ accent: "rose" }]);
    await waitFor(() => {
      expect(
        within(radioGroup("Accent color")).getByRole<HTMLInputElement>("radio", {
          name: /Instance default/,
        }).checked,
      ).toBe(true);
    });
    expect(
      within(radioGroup("Accent color")).getByRole<HTMLInputElement>("radio", { name: /Rose/ })
        .checked,
    ).toBe(false);
    expect(queryClient.getQueryData(qk.me.current())).toMatchObject({
      user: { accent: "default" },
    });
    expect(screen.queryByText("Saved")).toBeNull();
    expect(screen.queryByText(/disk full/)).toBeNull();
  });

  test("a language change persists and re-renders the application in that locale", async () => {
    const { state, user } = renderSettings("/settings/appearance");
    await screen.findByRole("heading", { level: 2, name: "Appearance" });

    await user.click(screen.getByRole("combobox"));
    await user.type(screen.getByRole("combobox"), "Deutsch");
    await user.click(await screen.findByTitle("Deutsch (Deutschland)"));

    expect(await screen.findByRole("heading", { level: 1, name: "Einstellungen" })).toBeDefined();
    expect(state.preferenceBodies).toEqual([{ locale: "de-DE" }]);
    expect(document.documentElement.lang).toBe("de-DE");
  });
});
