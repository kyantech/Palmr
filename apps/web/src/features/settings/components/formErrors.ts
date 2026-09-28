import { ApiError } from "../../../shared/errors";

export function invalidFields<Field extends string>(
  error: unknown,
  known: readonly Field[],
): Field[] {
  if (!(error instanceof ApiError) || error.code !== "VALIDATION_ERROR") {
    return [];
  }
  const fields = error.details.fields;
  return (Array.isArray(fields) ? fields : []).filter((field): field is Field =>
    (known as readonly string[]).includes(field),
  );
}

export function characters(value: string): number {
  return Array.from(value).length;
}
