import type { ApiError } from "../../../shared/errors";

export function ReadsApiErrorMessage({ error }: { error: ApiError }) {
  return <span>{error.code === "INTERNAL_ERROR" ? error.message : null}</span>;
}
