import { afterEach, beforeEach, describe, expect, expectTypeOf, test, vi } from "vitest";
import { ApiError } from "../errors";
import {
  apiFetch,
  type ApiFetch,
  type ApiPath,
  type ApiRoutes,
  type HttpMethod,
  type MethodOf,
} from "./apiFetch";
import { CSRF_COOKIE, CSRF_HEADER } from "./csrf";
import type { components } from "./schema";

interface Item {
  id: string;
  name: string;
}

interface NoParameters {
  query?: never;
  header?: never;
  path?: never;
  cookie?: never;
}

interface JsonResponse<Body> {
  headers: Record<string, unknown>;
  content: { "application/json": Body };
}

interface EmptyResponse {
  headers: Record<string, unknown>;
  content?: never;
}

interface ErrorResponses {
  400: JsonResponse<components["schemas"]["ApiErrorBody"]>;
}

interface Operation<Responses, Parameters = NoParameters> {
  parameters: Parameters;
  requestBody?: never;
  responses: Responses & ErrorResponses;
}

interface FixturePaths {
  "/api/v1/items": {
    parameters: NoParameters;
    get: Operation<
      { 200: JsonResponse<Item[]> },
      { query?: { cursor?: string; tag?: string[] }; header?: never; path?: never; cookie?: never }
    >;
    post: {
      parameters: NoParameters;
      requestBody: { content: { "application/json": { name: string } } };
      responses: { 201: JsonResponse<Item> } & ErrorResponses;
    };
    put: Operation<{ 200: JsonResponse<Item> }>;
    patch: Operation<{ 200: JsonResponse<Item> }>;
    delete: Operation<{ 204: EmptyResponse }>;
    head: Operation<{ 200: EmptyResponse }>;
    options: Operation<{ 204: EmptyResponse }>;
    trace?: never;
  };
  "/api/v1/items/{itemId}": {
    parameters: NoParameters;
    get: Operation<
      { 200: JsonResponse<Item> },
      { query?: never; header?: never; path: { itemId: string }; cookie?: never }
    >;
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
  "/health": {
    parameters: NoParameters;
    get: Operation<{ 200: JsonResponse<{ status: "ok" }> }>;
    put?: never;
    post?: never;
    delete?: never;
    options?: never;
    head?: never;
    patch?: never;
    trace?: never;
  };
}

type FixtureRoutes = ApiRoutes<FixturePaths>;

const fixtureFetch = apiFetch as unknown as ApiFetch<FixtureRoutes>;
const untypedFetch = apiFetch as unknown as (
  method: HttpMethod,
  path: string,
  options?: object,
) => Promise<unknown>;

const ALL_METHODS: readonly HttpMethod[] = [
  "get",
  "head",
  "options",
  "post",
  "put",
  "patch",
  "delete",
];

let baseElement: HTMLBaseElement | null = null;

function useBase(href: string) {
  baseElement = document.createElement("base");
  baseElement.href = href;
  document.head.append(baseElement);
}

function setCookie(pair: string) {
  document.cookie = `${pair}; path=/`;
}

function clearCookies() {
  for (const pair of document.cookie.split(";")) {
    const name = pair.split("=")[0]?.trim();
    if (name) {
      document.cookie = `${name}=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/`;
    }
  }
}

function json(body: unknown, init: ResponseInit = {}): Response {
  const headers = new Headers(init.headers);
  headers.set("Content-Type", "application/json; charset=utf-8");
  return new Response(JSON.stringify(body), { ...init, headers });
}

function stubFetch(respond: () => Response | Promise<Response>) {
  return vi.spyOn(globalThis, "fetch").mockImplementation(() => Promise.resolve(respond()));
}

function sentRequest(spy: ReturnType<typeof stubFetch>, index = 0) {
  const call = spy.mock.calls[index];
  if (!call) {
    throw new Error(`fetch call ${String(index)} was not made`);
  }
  const [url, init = {}] = call;
  if (typeof url !== "string") {
    throw new Error("apiFetch must pass fetch a relative URL string");
  }
  return { url, init, headers: new Headers(init.headers) };
}

async function rejection(promise: Promise<unknown>): Promise<ApiError> {
  const error: unknown = await promise.then(
    () => null,
    (reason: unknown) => reason,
  );
  if (!(error instanceof ApiError)) {
    throw new Error("expected the request to reject with an ApiError");
  }
  return error;
}

beforeEach(() => {
  clearCookies();
});

afterEach(() => {
  vi.restoreAllMocks();
  clearCookies();
  baseElement?.remove();
  baseElement = null;
});

test("unit_apiFetch_parses_error_envelope", async () => {
  const rawBody = {
    error: {
      code: "VALIDATION_ERROR",
      message: "The request is invalid",
      requestId: "019a0000-0000-7000-8000-000000000001",
      details: { fields: ["name", "email"], maxLength: 255, retryable: false },
    },
  };
  const response = json(rawBody, {
    status: 422,
    headers: { "X-Request-Id": "019a0000-0000-7000-8000-000000000001" },
  });
  stubFetch(() => response);

  const error = await rejection(fixtureFetch("post", "/items", { body: { name: "x" } }));

  expect(error).toBeInstanceOf(Error);
  expect(error.name).toBe("ApiError");
  expect(error.code).toBe("VALIDATION_ERROR");
  expect(error.status).toBe(422);
  expect(error.requestId).toBe("019a0000-0000-7000-8000-000000000001");
  expect(error.details).toEqual({ fields: ["name", "email"], maxLength: 255, retryable: false });
  expect(error.request).toEqual({ method: "POST", path: "/items" });
  expect(Object.keys(error).sort()).toEqual([
    "code",
    "details",
    "name",
    "request",
    "requestId",
    "status",
  ]);
  expect(Object.values(error)).not.toContain(response);
  expect(Object.values(error)).not.toContain(rawBody);
  expect(error.cause).toBeUndefined();
});

test("unit_apiFetch_sends_csrf_on_mutation", async () => {
  setCookie("palmr_csrf2=wrong");
  setCookie(`${CSRF_COOKIE}=csrf-token-value`);
  setCookie("xpalmr_csrf=also-wrong");
  const spy = stubFetch(() => new Response(null, { status: 204 }));

  for (const method of ALL_METHODS) {
    await untypedFetch(method, "/items");
  }

  ALL_METHODS.forEach((method, index) => {
    const { init, headers } = sentRequest(spy, index);
    expect(init.method).toBe(method.toUpperCase());
    expect(init.credentials).toBe("include");
    if (method === "post" || method === "put" || method === "patch" || method === "delete") {
      expect(headers.get(CSRF_HEADER), method).toBe("csrf-token-value");
    } else {
      expect(headers.has(CSRF_HEADER), method).toBe(false);
    }
  });
});

describe("request construction", () => {
  test("mutations without a CSRF cookie still send the header, empty", async () => {
    const spy = stubFetch(() => new Response(null, { status: 204 }));

    await fixtureFetch("delete", "/items");

    const { headers } = sentRequest(spy);
    expect(headers.has(CSRF_HEADER)).toBe(true);
    expect(headers.get(CSRF_HEADER)).toBe("");
  });

  test("callers cannot replace or inject the CSRF header", async () => {
    setCookie(`${CSRF_COOKIE}=canonical`);
    const spy = stubFetch(() => new Response(null, { status: 204 }));

    await fixtureFetch("delete", "/items", { headers: { [CSRF_HEADER]: "stale" } });
    await fixtureFetch("get", "/items", { headers: { "x-palmr-csrf": "stale" } });

    expect(sentRequest(spy, 0).headers.get(CSRF_HEADER)).toBe("canonical");
    expect(sentRequest(spy, 1).headers.has(CSRF_HEADER)).toBe(false);
  });

  test("credentials stay include whatever the caller passes", async () => {
    const spy = stubFetch(() => json([]));
    await untypedFetch("get", "/items", { credentials: "omit", method: "DELETE" });

    const { init } = sentRequest(spy);
    expect(init.credentials).toBe("include");
    expect(init.method).toBe("GET");
  });

  test("JSON bodies carry Content-Type; bodiless requests do not", async () => {
    const spy = stubFetch(() => json({ id: "1", name: "x" }, { status: 201 }));

    await fixtureFetch("post", "/items", { body: { name: "x" } });
    await fixtureFetch("get", "/items");

    const withBody = sentRequest(spy, 0);
    expect(withBody.headers.get("Content-Type")).toBe("application/json");
    expect(withBody.headers.get("Accept")).toBe("application/json");
    expect(withBody.init.body).toBe('{"name":"x"}');

    const withoutBody = sentRequest(spy, 1);
    expect(withoutBody.headers.has("Content-Type")).toBe(false);
    expect(withoutBody.headers.get("Accept")).toBe("application/json");
    expect(withoutBody.init.body).toBeUndefined();
  });

  test("caller headers are forwarded alongside the fixed ones", async () => {
    const spy = stubFetch(() => json([]));

    await fixtureFetch("get", "/items", { headers: { "Idempotency-Key": "k-1" } });

    expect(sentRequest(spy).headers.get("Idempotency-Key")).toBe("k-1");
  });

  test("the caller's AbortSignal reaches fetch untouched", async () => {
    const spy = stubFetch(() => json([]));
    const controller = new AbortController();

    await fixtureFetch("get", "/items", { signal: controller.signal });

    expect(sentRequest(spy).init.signal).toBe(controller.signal);
  });

  test("the URL is relative to the base path and never repeats the API prefix", async () => {
    useBase("/palmr/");
    const spy = stubFetch(() => json({ id: "a/b", name: "x" }));

    await fixtureFetch("get", "/items/{itemId}", { path: { itemId: "a/b?c#d" } });
    await fixtureFetch("get", "/items", { query: { cursor: "n&x", tag: ["a", "b"] } });

    expect(sentRequest(spy, 0).url).toBe("/palmr/api/v1/items/a%2Fb%3Fc%23d");
    expect(sentRequest(spy, 1).url).toBe("/palmr/api/v1/items?cursor=n%26x&tag=a&tag=b");
  });

  test("the URL at the root base path", async () => {
    const spy = stubFetch(() => json([]));

    await fixtureFetch("get", "/items");

    expect(sentRequest(spy).url).toBe("/api/v1/items");
  });

  test("path parameters cannot traverse out of their segment", async () => {
    const spy = stubFetch(() => json({}));

    for (const itemId of ["..", ".", ""]) {
      await expect(
        fixtureFetch("get", "/items/{itemId}", { path: { itemId } }),
      ).rejects.toBeInstanceOf(TypeError);
    }
    expect(spy).not.toHaveBeenCalled();
  });
});

describe("responses", () => {
  test("a JSON success body is returned", async () => {
    stubFetch(() => json({ id: "1", name: "first" }));

    const item = await fixtureFetch("get", "/items/{itemId}", { path: { itemId: "1" } });

    expectTypeOf(item).toEqualTypeOf<Item>();
    expect(item).toEqual({ id: "1", name: "first" });
  });

  test("204 resolves to undefined without parsing a body", async () => {
    const response = new Response(null, { status: 204 });
    const parse = vi.spyOn(response, "json");
    stubFetch(() => response);

    await expect(fixtureFetch("delete", "/items")).resolves.toBeUndefined();

    expectTypeOf(fixtureFetch<"/items", "delete">).returns.resolves.toEqualTypeOf<undefined>();
    expect(parse).not.toHaveBeenCalled();
  });

  test("HEAD resolves to undefined", async () => {
    stubFetch(() => new Response(null, { status: 200 }));

    await expect(fixtureFetch("head", "/items")).resolves.toBeUndefined();
  });

  test("a non-JSON success body is an unexpected response", async () => {
    stubFetch(
      () =>
        new Response("<!doctype html>", {
          status: 200,
          headers: { "Content-Type": "text/html", "X-Request-Id": "r-html" },
        }),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe("CLIENT_UNEXPECTED_RESPONSE");
    expect(error.status).toBe(200);
    expect(error.requestId).toBe("r-html");
  });
});

describe("request ids", () => {
  test("the X-Request-Id header is captured", async () => {
    stubFetch(() =>
      json(
        { error: { code: "NOT_FOUND", message: "", requestId: "from-body", details: {} } },
        { status: 404, headers: { "X-Request-Id": "from-header" } },
      ),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.requestId).toBe("from-header");
  });

  test("the envelope request id is used only when the header is absent", async () => {
    stubFetch(() =>
      json(
        { error: { code: "NOT_FOUND", message: "", requestId: "from-body", details: {} } },
        { status: 404 },
      ),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.requestId).toBe("from-body");
  });

  test("no request id anywhere is null", async () => {
    stubFetch(() => new Response("Bad Gateway", { status: 502 }));

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.requestId).toBeNull();
  });
});

describe("error classification", () => {
  test.each([
    [413, "CLIENT_PROXY_BODY_LIMIT"],
    [429, "CLIENT_RATE_LIMITED"],
    [502, "CLIENT_PROXY_BAD_GATEWAY"],
    [503, "CLIENT_PROXY_BAD_GATEWAY"],
    [504, "CLIENT_PROXY_TIMEOUT"],
    [500, "CLIENT_UNEXPECTED_RESPONSE"],
    [404, "CLIENT_UNEXPECTED_RESPONSE"],
  ])("a non-envelope %i becomes %s", async (status, code) => {
    stubFetch(
      () =>
        new Response("<html><body>proxy error</body></html>", {
          status,
          headers: { "Content-Type": "text/html" },
        }),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe(code);
    expect(error.status).toBe(status);
    expect(error.details).toEqual({});
    expect(error.message).not.toContain("proxy error");
  });

  test("JSON that is not the envelope is classified by status", async () => {
    stubFetch(() => json({ message: "upstream timed out" }, { status: 504 }));

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe("CLIENT_PROXY_TIMEOUT");
    expect(error.message).not.toContain("upstream");
  });

  test("a malformed JSON error body is classified by status", async () => {
    stubFetch(
      () => new Response("{", { status: 413, headers: { "Content-Type": "application/json" } }),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe("CLIENT_PROXY_BODY_LIMIT");
  });

  test.each([413, 429, 502, 503, 504])(
    "a Palmr envelope on %i keeps its server code",
    async (status) => {
      stubFetch(() =>
        json(
          { error: { code: "SERVICE_UNAVAILABLE", message: "m", requestId: "r", details: {} } },
          { status },
        ),
      );

      const error = await rejection(fixtureFetch("get", "/items"));

      expect(error.code).toBe("SERVICE_UNAVAILABLE");
    },
  );

  test("an envelope cannot claim a client-reserved code", async () => {
    stubFetch(() =>
      json(
        { error: { code: "CLIENT_OFFLINE", message: "m", requestId: "r", details: {} } },
        { status: 502 },
      ),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe("CLIENT_PROXY_BAD_GATEWAY");
  });

  test("a server code unknown to this build still wins", async () => {
    stubFetch(() =>
      json(
        { error: { code: "SOME_FUTURE_CODE", message: "m", requestId: "r", details: {} } },
        { status: 409 },
      ),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe("SOME_FUTURE_CODE");
  });

  test("detail values outside the schema are dropped", async () => {
    stubFetch(() =>
      json(
        {
          error: {
            code: "VALIDATION_ERROR",
            message: "m",
            requestId: "r",
            details: {
              kept: 1,
              fields: ["name"],
              nested: { a: 1 },
              list: [1],
              mixed: ["a", 1],
            },
          },
        },
        { status: 400 },
      ),
    );

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.details).toEqual({ kept: 1, fields: ["name"] });
  });

  test("an aborted request is CLIENT_ABORTED", async () => {
    const controller = new AbortController();
    vi.spyOn(globalThis, "fetch").mockImplementation(() => {
      controller.abort();
      return Promise.reject(new DOMException("The operation was aborted.", "AbortError"));
    });

    const error = await rejection(fixtureFetch("get", "/items", { signal: controller.signal }));

    expect(error.code).toBe("CLIENT_ABORTED");
    expect(error.status).toBe(0);
    expect(error.requestId).toBeNull();
  });

  test("a rejected fetch while offline is CLIENT_OFFLINE", async () => {
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new TypeError("Failed to fetch"));

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe("CLIENT_OFFLINE");
    expect(error.status).toBe(0);
  });

  test("a rejected fetch while online is CLIENT_NETWORK_ERROR", async () => {
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(true);
    const failure = new TypeError("Failed to fetch");
    vi.spyOn(globalThis, "fetch").mockRejectedValue(failure);

    const error = await rejection(fixtureFetch("get", "/items"));

    expect(error.code).toBe("CLIENT_NETWORK_ERROR");
    expect(error.cause).toBe(failure);
  });
});

test("route types follow the generated schema", () => {
  expectTypeOf<keyof FixtureRoutes>().toEqualTypeOf<"/items" | "/items/{itemId}">();
  expectTypeOf<MethodOf<FixtureRoutes["/items/{itemId}"]>>().toEqualTypeOf<"get">();
  expectTypeOf<Extract<ApiPath, "/health" | `/api/v1${string}`>>().toBeNever();
  expectTypeOf<ApiPath>().not.toEqualTypeOf<string>();

  const misuse = [
    // @ts-expect-error a path outside the JSON API is not callable
    () => fixtureFetch("get", "/health"),
    // @ts-expect-error the full OpenAPI key is not the API-relative path
    () => fixtureFetch("get", "/api/v1/items"),
    // @ts-expect-error the method is not declared for this path
    () => fixtureFetch("post", "/items/{itemId}", { path: { itemId: "1" } }),
    // @ts-expect-error required path parameters must be supplied
    () => fixtureFetch("get", "/items/{itemId}"),
    // @ts-expect-error a required JSON body must be supplied
    () => fixtureFetch("post", "/items"),
    // @ts-expect-error a body is rejected where none is declared
    () => fixtureFetch("get", "/items", { body: { name: "x" } }),
    // @ts-expect-error health is served outside the JSON API
    () => apiFetch("get", "/health"),
  ];
  expect(misuse).toHaveLength(7);
});
