export function apiFetch(path: string): Promise<Response> {
  return fetch(path);
}
