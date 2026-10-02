import { ApiError, detailFields } from "../../../shared/errors";

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

export function characters(value: string): number {
  return Array.from(value).length;
}
