import {
  ApiError,
  type ApiRequestDescription,
  type ClientErrorCode,
  type ErrorCode,
  type ErrorDetails,
} from "../errors";
import { API_PREFIX, resolveApiBase } from "./basePath";
import { CSRF_COOKIE, CSRF_HEADER, readCookie } from "./csrf";
import type { paths } from "./schema";

export const REQUEST_ID_HEADER = "X-Request-Id";

const JSON_MEDIA_TYPE = "application/json";
const SERVER_ERROR_CODE = /^[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)*$/;

export type HttpMethod = "get" | "put" | "post" | "delete" | "options" | "head" | "patch";

type Defined<T> = Exclude<T, undefined>;
type IsNever<T> = [T] extends [never] ? true : false;
type AllOptional<T> = Partial<T> extends T ? true : false;

export type ApiRoutes<Paths = paths> = {
  [
    K in keyof Paths as K extends `${typeof API_PREFIX}${infer Route extends `/${string}`}`
      ? Route
      : never
  ]: Paths[K];
};

export type ApiPath = Extract<keyof ApiRoutes, string>;

type OperationOf<Item, M> = M extends keyof Item ? Defined<Item[M]> : never;

export type MethodOf<Item> = {
  [M in HttpMethod]: IsNever<OperationOf<Item, M>> extends true ? never : M;
}[HttpMethod];

type JsonContent<T> = T extends { content: { "application/json": infer Body } } ? Body : undefined;

type SuccessStatus = 200 | 201 | 202 | 203 | 204 | 205 | 206;

export type SuccessOf<Operation> = Operation extends { responses: infer Responses }
  ? { [S in keyof Responses & SuccessStatus]: JsonContent<Responses[S]> }[keyof Responses &
      SuccessStatus]
  : never;

type ParameterGroup<Operation, Where extends "path" | "query"> = Operation extends {
  parameters: infer Parameters;
}
  ? Where extends keyof Parameters
    ? Defined<Parameters[Where]>
    : never
  : never;

type ParameterOption<Operation, Where extends "path" | "query"> =
  IsNever<ParameterGroup<Operation, Where>> extends true
    ? Partial<Record<Where, never>>
    : AllOptional<ParameterGroup<Operation, Where>> extends true
      ? Partial<Record<Where, ParameterGroup<Operation, Where>>>
      : Record<Where, ParameterGroup<Operation, Where>>;

type BodyOption<Operation> = Operation extends { requestBody?: infer RequestBody }
  ? IsNever<Defined<RequestBody>> extends true
    ? { body?: never }
    : undefined extends RequestBody
      ? { body?: JsonContent<Defined<RequestBody>> }
      : { body: JsonContent<RequestBody> }
  : { body?: never };

export type ApiRequestOptions<Operation> = {
  headers?: HeadersInit;
  signal?: AbortSignal;
} & ParameterOption<Operation, "path"> &
  ParameterOption<Operation, "query"> &
  BodyOption<Operation>;

type OptionsArgument<Operation> =
  AllOptional<ApiRequestOptions<Operation>> extends true
    ? [options?: ApiRequestOptions<Operation>]
    : [options: ApiRequestOptions<Operation>];

export type ApiFetch<Routes> = <P extends keyof Routes & string, M extends MethodOf<Routes[P]>>(
  method: M,
  path: P,
  ...options: OptionsArgument<OperationOf<Routes[P], M>>
) => Promise<SuccessOf<OperationOf<Routes[P], M>>>;

type PathValue = string | number;
type QueryValue = string | number | boolean;

interface RequestShape {
  headers?: HeadersInit;
  signal?: AbortSignal;
  path?: Readonly<Record<string, PathValue>>;
  query?: Readonly<Record<string, QueryValue | readonly QueryValue[] | undefined>>;
  body?: unknown;
}

export function isMutationMethod(method: HttpMethod): boolean {
  return method === "post" || method === "put" || method === "patch" || method === "delete";
}

async function sendApiRequest(
  method: HttpMethod,
  path: string,
  options: RequestShape = {},
): Promise<unknown> {
  const request: ApiRequestDescription = { method: method.toUpperCase(), path };
  const url = `${resolveApiBase()}${interpolatePath(path, options.path)}${toQueryString(options.query)}`;

  const headers = new Headers(options.headers);
  headers.set("Accept", JSON_MEDIA_TYPE);
  const init: RequestInit = { method: request.method, headers, credentials: "include" };
  if (options.body !== undefined) {
    headers.set("Content-Type", JSON_MEDIA_TYPE);
    init.body = JSON.stringify(options.body);
  }
  if (isMutationMethod(method)) {
    headers.set(CSRF_HEADER, readCookie(CSRF_COOKIE) ?? "");
  } else {
    headers.delete(CSRF_HEADER);
  }
  if (options.signal) {
    init.signal = options.signal;
  }

  let response: Response;
  try {
    response = await fetch(url, init);
  } catch (error) {
    throw transportError(error, options.signal, request);
  }

  const requestId = response.headers.get(REQUEST_ID_HEADER);
  const context: ResponseContext = { request, requestId, signal: options.signal };
  if (response.ok) {
    return readSuccess(response, method, context);
  }
  throw await readFailure(response, context);
}

// The runtime request does not depend on the route map, and TypeScript cannot
// relate a plain string path back to its generated entry inside a generic body,
// so the generated route types are attached once, here.
export const apiFetch = sendApiRequest as ApiFetch<ApiRoutes>;

function interpolatePath(template: string, values: RequestShape["path"] = {}): string {
  return template.replace(/\{([^{}]+)\}/g, (_placeholder, name: string) => {
    const value = values[name];
    if (value === undefined) {
      throw new TypeError(`Missing path parameter "${name}" for ${template}`);
    }
    const segment = String(value);
    if (segment === "" || segment === "." || segment === "..") {
      throw new TypeError(`Path parameter "${name}" must be a non-empty segment`);
    }
    return encodeURIComponent(segment);
  });
}

function toQueryString(query: RequestShape["query"]): string {
  if (!query) {
    return "";
  }
  const search = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    const values = typeof value === "object" ? value : value === undefined ? [] : [value];
    for (const item of values) {
      search.append(name, String(item));
    }
  }
  const encoded = search.toString();
  return encoded === "" ? "" : `?${encoded}`;
}

interface ResponseContext {
  request: ApiRequestDescription;
  requestId: string | null;
  signal: AbortSignal | undefined;
}

async function readSuccess(
  response: Response,
  method: HttpMethod,
  context: ResponseContext,
): Promise<unknown> {
  if (response.status === 204 || response.status === 205 || method === "head") {
    await discardBody(response);
    return undefined;
  }
  if (!isJsonResponse(response)) {
    await discardBody(response);
    throw clientError("CLIENT_UNEXPECTED_RESPONSE", response.status, context);
  }
  try {
    return await response.json();
  } catch (error) {
    throw bodyReadError(error, response.status, context);
  }
}

async function readFailure(response: Response, context: ResponseContext): Promise<ApiError> {
  if (!isJsonResponse(response)) {
    await discardBody(response);
    return clientError(proxyErrorCode(response.status), response.status, context);
  }
  let body: unknown;
  try {
    body = await response.json();
  } catch (error) {
    return bodyReadError(error, response.status, context, proxyErrorCode(response.status));
  }
  return (
    envelopeError(body, response.status, context) ??
    clientError(proxyErrorCode(response.status), response.status, context)
  );
}

function envelopeError(body: unknown, status: number, context: ResponseContext): ApiError | null {
  if (!isRecord(body) || !isRecord(body.error)) {
    return null;
  }
  const { code, message, requestId, details } = body.error;
  if (typeof code !== "string" || !isServerErrorCode(code)) {
    return null;
  }
  return new ApiError({
    code,
    status,
    requestId: context.requestId ?? (typeof requestId === "string" ? requestId : null),
    details: toDetails(details),
    request: context.request,
    serverMessage: typeof message === "string" ? message : code,
  });
}

// The generated union only describes codes this build knows about; a newer
// server may send one it does not, and that code must still win over any
// status-based guess.
function isServerErrorCode(code: string): code is ErrorCode {
  return SERVER_ERROR_CODE.test(code) && !code.startsWith("CLIENT_");
}

function toDetails(value: unknown): ErrorDetails {
  if (!isRecord(value)) {
    return {};
  }
  const details: Record<string, boolean | number | string | string[]> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (typeof entry === "boolean" || typeof entry === "number" || typeof entry === "string") {
      details[key] = entry;
    } else if (isStringList(entry)) {
      details[key] = [...entry];
    }
  }
  return details;
}

function isStringList(value: unknown): value is readonly string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function proxyErrorCode(status: number): ClientErrorCode {
  switch (status) {
    case 413:
      return "CLIENT_PROXY_BODY_LIMIT";
    case 429:
      return "CLIENT_RATE_LIMITED";
    case 502:
    case 503:
      return "CLIENT_PROXY_BAD_GATEWAY";
    case 504:
      return "CLIENT_PROXY_TIMEOUT";
    default:
      return "CLIENT_UNEXPECTED_RESPONSE";
  }
}

function transportError(
  error: unknown,
  signal: AbortSignal | undefined,
  request: ApiRequestDescription,
): ApiError {
  const code: ClientErrorCode = signal?.aborted
    ? "CLIENT_ABORTED"
    : navigator.onLine
      ? "CLIENT_NETWORK_ERROR"
      : "CLIENT_OFFLINE";
  return clientError(code, 0, { request, requestId: null }, error);
}

function bodyReadError(
  error: unknown,
  status: number,
  context: ResponseContext,
  fallback: ClientErrorCode = "CLIENT_UNEXPECTED_RESPONSE",
): ApiError {
  if (context.signal?.aborted) {
    return clientError("CLIENT_ABORTED", status, context, error);
  }
  return clientError(
    error instanceof SyntaxError ? fallback : "CLIENT_NETWORK_ERROR",
    status,
    context,
    error,
  );
}

function clientError(
  code: ClientErrorCode,
  status: number,
  context: Pick<ResponseContext, "request" | "requestId">,
  cause?: unknown,
): ApiError {
  return new ApiError({
    code,
    status,
    requestId: context.requestId,
    details: {},
    request: context.request,
    serverMessage: code,
    cause,
  });
}

function isJsonResponse(response: Response): boolean {
  const mediaType = (response.headers.get("Content-Type") ?? "")
    .split(";")[0]
    ?.trim()
    .toLowerCase();
  return mediaType === JSON_MEDIA_TYPE || (mediaType?.endsWith("+json") ?? false);
}

async function discardBody(response: Response): Promise<void> {
  try {
    await response.body?.cancel();
  } catch {
    return;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
