import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ConfigProvider } from "antd";
import type { i18n as I18n } from "i18next";
import { StrictMode } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { ApiError } from "../../shared/errors";
import type { LocaleCode } from "../i18n/catalog";
import { createI18n, preloadEagerNamespaces } from "../i18n/i18n";
import { RootErrorBoundary } from "./RootErrorBoundary";
import { RouteErrorBoundary } from "./RouteErrorBoundary";
import { createStaleChunkRecovery, STALE_CHUNK_GUARD_WINDOW_MS } from "./staleChunk";
import { memoryEnvironment } from "../../test/staleChunkEnvironment";

function Throws({ error }: { error: unknown }): never {
  throw error;
}

const staleChunk = () =>
  new TypeError(
    "Failed to fetch dynamically imported module: https://palmr.test/assets/Files-3f9a.js",
  );

async function loadedI18n(locale: LocaleCode): Promise<I18n> {
  const i18n = createI18n();
  await preloadEagerNamespaces(i18n, locale);
  await i18n.changeLanguage(locale);
  return i18n;
}

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("RootErrorBoundary", () => {
  test("renders an accessible recovery panel with no provider above it", async () => {
    const memory = memoryEnvironment();
    const recovery = createStaleChunkRecovery(memory.environment);

    render(
      <RootErrorBoundary recovery={recovery}>
        <Throws error={new Error("QueryClient construction failed")} />
      </RootErrorBoundary>,
    );

    const alert = screen.getByRole("alert");
    expect(alert).toHaveProperty("textContent", expect.stringContaining("Something went wrong"));
    await userEvent.click(screen.getByRole("button", { name: "Reload" }));
    expect(memory.reload).toHaveBeenCalledTimes(1);
    expect(memory.store.size).toBe(0);
  });

  test("a normal render error never reloads automatically", () => {
    const memory = memoryEnvironment();

    render(
      <RootErrorBoundary recovery={createStaleChunkRecovery(memory.environment)}>
        <Throws error={new TypeError("Cannot read properties of undefined")} />
      </RootErrorBoundary>,
    );

    expect(screen.getByRole("alert")).toBeDefined();
    expect(memory.reload).not.toHaveBeenCalled();
  });

  test("the first stale chunk reloads once, even under StrictMode, and shows no panel", () => {
    const memory = memoryEnvironment();

    render(
      <StrictMode>
        <RootErrorBoundary recovery={createStaleChunkRecovery(memory.environment)}>
          <Throws error={staleChunk()} />
        </RootErrorBoundary>
      </StrictMode>,
    );

    expect(memory.reload).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("a second stale chunk inside the window shows the panel instead of looping", async () => {
    const memory = memoryEnvironment();
    render(
      <RootErrorBoundary recovery={createStaleChunkRecovery(memory.environment)}>
        <Throws error={staleChunk()} />
      </RootErrorBoundary>,
    );
    memory.advance(1_000);

    render(
      <RootErrorBoundary recovery={createStaleChunkRecovery(memory.environment)}>
        <Throws error={staleChunk()} />
      </RootErrorBoundary>,
    );

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Palmr was updated");
    expect(memory.reload).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Reload" }));
    expect(memory.reload).toHaveBeenCalledTimes(2);
  });

  test("after the window expires a stale chunk may reload once more", () => {
    const memory = memoryEnvironment();
    createStaleChunkRecovery(memory.environment).recover(staleChunk());
    memory.advance(STALE_CHUNK_GUARD_WINDOW_MS + 1);

    render(
      <RootErrorBoundary recovery={createStaleChunkRecovery(memory.environment)}>
        <Throws error={staleChunk()} />
      </RootErrorBoundary>,
    );

    expect(memory.reload).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("the request id is shown when the failure carries one", () => {
    const failure = new ApiError({
      code: "INTERNAL_ERROR",
      status: 500,
      requestId: "019a0000-0000-7000-8000-00000000abcd",
      details: {},
      request: { method: "GET", path: "/bootstrap" },
      serverMessage: "internal",
    });

    render(
      <RootErrorBoundary recovery={createStaleChunkRecovery(memoryEnvironment().environment)}>
        <Throws error={failure} />
      </RootErrorBoundary>,
    );

    expect(screen.getByRole("alert").textContent).toContain(
      "Request ID: 019a0000-0000-7000-8000-00000000abcd",
    );
  });

  test("uses loaded translations without depending on React i18n context", async () => {
    const i18n = await loadedI18n("pt-BR");

    render(
      <RootErrorBoundary
        i18n={i18n}
        recovery={createStaleChunkRecovery(memoryEnvironment().environment)}
      >
        <Throws error={new Error("boom")} />
      </RootErrorBoundary>,
    );

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Algo deu errado");
    expect(screen.getByRole("button", { name: "Recarregar" })).toBeDefined();
  });
});

describe("RouteErrorBoundary", () => {
  async function renderRoute(
    error: unknown,
    recovery: ReturnType<typeof createStaleChunkRecovery>,
  ) {
    const i18n = await loadedI18n("en-US");
    return render(
      <I18nextProvider i18n={i18n}>
        <ConfigProvider>
          <RouteErrorBoundary recovery={recovery}>
            <Throws error={error} />
          </RouteErrorBoundary>
        </ConfigProvider>
      </I18nextProvider>,
    );
  }

  test("a normal route error renders the panel with a Reload action and no reload", async () => {
    const memory = memoryEnvironment();

    await renderRoute(
      new Error("route render failed"),
      createStaleChunkRecovery(memory.environment),
    );

    expect(await screen.findByText("Something went wrong")).toBeDefined();
    expect(memory.reload).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Reload" }));
    expect(memory.reload).toHaveBeenCalledTimes(1);
  });

  test("a stale route chunk reloads once, then shows the update panel", async () => {
    const memory = memoryEnvironment();

    const first = await renderRoute(staleChunk(), createStaleChunkRecovery(memory.environment));
    await waitFor(() => {
      expect(memory.reload).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByText("Palmr was updated")).toBeNull();
    first.unmount();

    await renderRoute(staleChunk(), createStaleChunkRecovery(memory.environment));
    expect(await screen.findByText("Palmr was updated")).toBeDefined();
    expect(memory.reload).toHaveBeenCalledTimes(1);
  });

  test("shows the request id of an API failure", async () => {
    const failure = new ApiError({
      code: "SERVICE_UNAVAILABLE",
      status: 503,
      requestId: "req-42",
      details: {},
      request: { method: "GET", path: "/files" },
      serverMessage: "unavailable",
    });

    await renderRoute(failure, createStaleChunkRecovery(memoryEnvironment().environment));

    expect(await screen.findByText("Request ID: req-42")).toBeDefined();
  });
});
