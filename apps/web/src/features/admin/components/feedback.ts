import { ApiError, detailFields, presentError } from "../../../shared/errors";

export function reportable(error: unknown): unknown {
  const presented = presentError(error);
  return presented.presentation.silent || presented.code === "AUTH_RECENT_AUTH_REQUIRED"
    ? null
    : error;
}

export function invalidFields<Field extends string>(
  error: unknown,
  known: readonly Field[],
): Field[] {
  if (!(error instanceof ApiError) || error.code !== "VALIDATION_ERROR") {
    return [];
  }
  return detailFields(error).filter((field): field is Field =>
    (known as readonly string[]).includes(field),
  );
}

export function detailNumber(error: ApiError, key: string): number | null {
  const value = error.details[key];
  return typeof value === "number" ? value : null;
}

export function detailText(error: ApiError, key: string): string | null {
  const value = error.details[key];
  return typeof value === "string" ? value : null;
}

export function newIdempotencyKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
