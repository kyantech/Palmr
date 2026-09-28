import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { App } from "./App";

afterEach(() => {
  document.head.querySelectorAll('meta[name="csp-nonce"]').forEach((meta) => {
    meta.remove();
  });
  vi.resetModules();
});

test("the app reads the shell nonce before its first AntD render and resolves one locale", async () => {
  document.head.insertAdjacentHTML("beforeend", '<meta name="csp-nonce" content="sh3ll">');
  vi.resetModules();
  const { App: FreshApp } = await import("./App");

  render(<FreshApp />);

  await screen.findByRole("heading", { level: 1 });
  const styles = [...document.head.querySelectorAll("style")];
  expect(styles.length).toBeGreaterThan(0);
  expect(styles.every((style) => style.getAttribute("nonce") === "sh3ll")).toBe(true);
  await waitFor(() => {
    expect(document.documentElement.lang).toBe("en-US");
  });
  expect(document.documentElement.dir).toBe("ltr");
});

test("unit_app_renders", async () => {
  render(<App />);

  expect(await screen.findByRole("heading", { level: 1 })).toBeDefined();
});
