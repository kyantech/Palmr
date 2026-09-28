import { render, screen, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { bootHandlers, bootstrapFixture } from "../test/bootFixtures";
import { stubMatchMedia } from "../test/renderSession";
import { server } from "../test/server";

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  document.head.querySelectorAll('meta[name="csp-nonce"]').forEach((meta) => {
    meta.remove();
  });
  window.history.replaceState(null, "", "/");
  vi.unstubAllGlobals();
  vi.resetModules();
});

test("the app reads the shell nonce before its first AntD render and resolves one locale", async () => {
  server.use(...bootHandlers().handlers);
  document.head.insertAdjacentHTML("beforeend", '<meta name="csp-nonce" content="sh3ll">');
  vi.resetModules();
  const { App: FreshApp } = await import("./App");

  render(<FreshApp />);

  await screen.findByRole("heading", { level: 1, name: "Sign in" });
  expect(window.location.pathname).toBe("/login");
  const styles = [...document.head.querySelectorAll("style")];
  expect(styles.length).toBeGreaterThan(0);
  expect(styles.every((style) => style.getAttribute("nonce") === "sh3ll")).toBe(true);
  await waitFor(() => {
    expect(document.documentElement.lang).toBe("en-US");
  });
  expect(document.documentElement.dir).toBe("ltr");
});

test("unit_app_renders", async () => {
  const { calls, handlers } = bootHandlers({
    bootstrap: bootstrapFixture({ setupCompleted: false }),
  });
  server.use(
    ...handlers,
    http.get("*/api/v1/setup/status", () =>
      HttpResponse.json({ setupCompleted: false, passwordMinLength: 8 }),
    ),
  );
  vi.resetModules();
  const { App: FreshApp } = await import("./App");

  render(<FreshApp />);

  expect(
    await screen.findByRole("heading", { level: 1, name: "Set up your instance" }),
  ).toBeDefined();
  expect(window.location.pathname).toBe("/setup");
  expect(calls).toEqual({ bootstrap: 1, me: 0 });
});
