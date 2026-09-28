export { ApiError, type ApiErrorInit, type ApiRequestDescription } from "./ApiError";
export type { ClientErrorCode, ErrorCode, ErrorDetails, ServerErrorCode } from "./codes";
export { ErrorAlert, ErrorTechnicalDetails, RequestId, useErrorMessage } from "./ErrorView";
export {
  ERROR_PRESENTATION,
  type ErrorMessageKey,
  type ErrorPresentation,
  type ErrorSeverity,
  type ErrorSurface,
  isApiErrorCode,
  isKnownErrorCode,
  type PresentedError,
  presentError,
  UNKNOWN_ERROR_PRESENTATION,
} from "./presentation";
