import { apiFetch } from "../../../shared/api";

export function CallsApiFetch() {
  return <span>{apiFetch.name}</span>;
}
