export class ApiError extends Error {
  readonly code: string = "INTERNAL_ERROR";
}
