import { type ReportedError, reportedError } from "../../shared/errors";

export const CALLBACK_ERROR_PARAM = "error";
export const CALLBACK_REQUEST_ID_PARAM = "requestId";

export function readCallbackError(search: string): ReportedError | null {
  const params = new URLSearchParams(search);
  return reportedError(params.get(CALLBACK_ERROR_PARAM), params.get(CALLBACK_REQUEST_ID_PARAM));
}

export function hasCallbackParams(search: string): boolean {
  const params = new URLSearchParams(search);
  return params.has(CALLBACK_ERROR_PARAM) || params.has(CALLBACK_REQUEST_ID_PARAM);
}

export function withoutCallbackParams(search: string): string {
  const params = new URLSearchParams(search);
  params.delete(CALLBACK_ERROR_PARAM);
  params.delete(CALLBACK_REQUEST_ID_PARAM);
  const rest = params.toString();
  return rest === "" ? "" : `?${rest}`;
}
