import { render, screen } from "@testing-library/react";
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { I18nextProvider } from "react-i18next";
import { describe, expect, expectTypeOf, test } from "vitest";
import { loadedI18n } from "../../test/renderRouter";
import { ApiError } from "./ApiError";
import type { ErrorCode } from "./codes";
import { ErrorAlert } from "./ErrorView";
import { ERROR_PRESENTATION, presentError, UNKNOWN_ERROR_PRESENTATION } from "./presentation";

const webRoot = resolve(import.meta.dirname, "../../..");
const localesRoot = join(webRoot, "src/app/i18n/locales");

function unionMembers(source: string, declaration: RegExp): string[] {
  const match = declaration.exec(source);
  if (!match?.[1]) {
    throw new Error(`declaration ${String(declaration)} not found`);
  }
  return [...match[1].matchAll(/"([A-Z0-9_]+)"/g)].map(([, code]) => code ?? "");
}

const generatedServerCodes = unionMembers(
  readFileSync(join(webRoot, "src/shared/api/schema.d.ts"), "utf8"),
  /^\s*ErrorCode: ((?:\s*\|?\s*"[A-Z0-9_]+")+);/m,
);
const clientCodes = unionMembers(
  readFileSync(join(webRoot, "src/shared/errors/codes.ts"), "utf8"),
  /export type ClientErrorCode =((?:\s*\|\s*"[A-Z0-9_]+")+);/,
);

function readErrors(locale: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(localesRoot, locale, "errors.json"), "utf8")) as Record<
    string,
    unknown
  >;
}

function lookup(tree: Record<string, unknown>, dotted: string): unknown {
  return dotted
    .split(".")
    .reduce<unknown>(
      (node, part) =>
        typeof node === "object" && node !== null
          ? (node as Record<string, unknown>)[part]
          : undefined,
      tree,
    );
}

function apiError(code: string, requestId: string | null, serverMessage: string) {
  return new ApiError({
    code: code as ErrorCode,
    status: 500,
    requestId,
    details: {},
    request: { method: "POST", path: "/probe" },
    serverMessage,
  });
}

async function renderAlert(error: unknown) {
  const i18n = await loadedI18n();
  return render(
    <I18nextProvider i18n={i18n}>
      <ErrorAlert error={error} />
    </I18nextProvider>,
  );
}

describe("unit_every_error_code_has_i18n_key", () => {
  test("the map is keyed by exactly the current generated server codes plus client codes", () => {
    expectTypeOf<keyof typeof ERROR_PRESENTATION>().toEqualTypeOf<ErrorCode>();
    expect(generatedServerCodes.length).toBeGreaterThan(0);
    expect(clientCodes.length).toBeGreaterThan(0);
    expect(new Set(generatedServerCodes).size).toBe(generatedServerCodes.length);

    const known = [...generatedServerCodes, ...clientCodes].sort();
    expect(Object.keys(ERROR_PRESENTATION).sort()).toEqual(known);
  });

  test("every presentation entry references a real en-US errors key, present in every locale", () => {
    const english = readErrors("en-US");
    const entries = [...Object.values(ERROR_PRESENTATION), UNKNOWN_ERROR_PRESENTATION];
    for (const { i18nKey } of entries) {
      const text = lookup(english, i18nKey);
      expect(typeof text, i18nKey).toBe("string");
      expect((text as string).trim(), i18nKey).not.toBe("");
    }

    const locales = readdirSync(localesRoot);
    expect(locales).toHaveLength(23);
    for (const locale of locales) {
      const catalogue = readErrors(locale);
      for (const { i18nKey } of entries) {
        expect(typeof lookup(catalogue, i18nKey), `${locale}:${i18nKey}`).toBe("string");
      }
    }
  });

  test("no orphan message keys: every en-US message is used by a presentation entry", () => {
    const english = readErrors("en-US");
    const used = new Set(
      [...Object.values(ERROR_PRESENTATION), UNKNOWN_ERROR_PRESENTATION].map(
        ({ i18nKey }) => i18nKey,
      ),
    );
    const defined = Object.keys(english.message as Record<string, string>).map(
      (key) => `message.${key}`,
    );
    expect(defined.sort()).toEqual([...used].sort());
  });

  test("modal and page surfaces always expose the request id", () => {
    for (const [code, presentation] of Object.entries(ERROR_PRESENTATION)) {
      if (presentation.surface === "modal" || presentation.surface === "page") {
        expect(presentation.showRequestId, code).toBe(true);
      }
    }
    expect(UNKNOWN_ERROR_PRESENTATION.showRequestId).toBe(true);
  });

  test("presentation retryability follows the API catalogue, not the automatic retry policy", () => {
    expect(ERROR_PRESENTATION.IDEMPOTENCY_REQUEST_IN_PROGRESS.retryable).toBe(true);
    expect(ERROR_PRESENTATION.AUTH_LOCKED.retryable).toBe(true);
    expect(ERROR_PRESENTATION.AUTH_REQUIRED.retryable).toBe(false);
    expect(ERROR_PRESENTATION.AUTH_RECENT_AUTH_REQUIRED.surface).toBe("modal");
    expect(ERROR_PRESENTATION.CLIENT_ABORTED.silent).toBe(true);
    expect(
      Object.entries(ERROR_PRESENTATION)
        .filter(([, presentation]) => presentation.silent)
        .map(([code]) => code),
    ).toEqual(["CLIENT_ABORTED"]);
  });

  test("an unknown runtime code uses the generic fallback, shows the raw code and request id, never the server message", async () => {
    const error = apiError("QUOTA_EXCEEDED_V9", "req-unknown", "Quota exceeded: 5 GB of 5 GB");
    const presented = presentError(error);
    expect(presented).toMatchObject({
      presentation: UNKNOWN_ERROR_PRESENTATION,
      code: "QUOTA_EXCEEDED_V9",
      known: false,
      requestId: "req-unknown",
    });

    await renderAlert(error);

    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain(
      "Palmr couldn't complete this action. If the problem continues, contact your administrator and include the details below.",
    );
    expect(alert.textContent).toContain("Error code: QUOTA_EXCEEDED_V9");
    expect(alert.textContent).toContain("Request ID: req-unknown");
    expect(alert.textContent).not.toContain("Quota exceeded: 5 GB");
    expect(alert.textContent).not.toMatch(/^Unknown error$/);
  });

  test("a known code renders its localized message, never the server prose", async () => {
    await renderAlert(apiError("INTERNAL_ERROR", "req-1", "sqlite: disk image is malformed"));

    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Palmr ran into an unexpected problem.");
    expect(alert.textContent).toContain("Request ID: req-1");
    expect(alert.textContent).not.toContain("sqlite");
    expect(alert.textContent).not.toContain("Error code");
  });

  test("a non-API error still renders the generic fallback without inventing a request id", async () => {
    await renderAlert(new TypeError("undefined is not a function"));

    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Palmr couldn't complete this action.");
    expect(alert.textContent).not.toContain("undefined is not a function");
    expect(alert.textContent).not.toContain("Request ID");
  });
});

describe("component_error_request_id", () => {
  test("the request id is shown when present", async () => {
    await renderAlert(apiError("CLIENT_PROXY_BODY_LIMIT", "req-413", "CLIENT_PROXY_BODY_LIMIT"));

    const requestId = screen.getByText("Request ID: req-413");
    expect(requestId.closest("[data-request-id]")?.getAttribute("data-request-id")).toBe("req-413");
    expect(screen.getByRole("button", { name: /copy/i })).toBeDefined();
  });

  test("the request id is omitted cleanly when null", async () => {
    await renderAlert(apiError("CLIENT_PROXY_BODY_LIMIT", null, "CLIENT_PROXY_BODY_LIMIT"));

    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("A proxy in front of Palmr rejected the request");
    expect(alert.textContent).not.toContain("Request ID");
    expect(alert.querySelector("[data-request-id]")).toBeNull();
  });

  test("an expected abort renders nothing", async () => {
    const { container } = await renderAlert(apiError("CLIENT_ABORTED", null, "CLIENT_ABORTED"));
    expect(container.textContent).toBe("");
  });
});
