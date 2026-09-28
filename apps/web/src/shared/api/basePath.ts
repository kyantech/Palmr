export const API_PREFIX = "/api/v1";

export function resolveBasename(baseURI: string = document.baseURI): string {
  const { pathname } = new URL(baseURI);
  return pathname.replace(/\/+$/, "") || "/";
}

export function resolveApiBase(baseURI?: string): string {
  const basename = resolveBasename(baseURI);
  return basename === "/" ? API_PREFIX : `${basename}${API_PREFIX}`;
}

export function resolveApiUrl(url: string, baseURI?: string): string {
  return url === API_PREFIX || url.startsWith(`${API_PREFIX}/`)
    ? `${resolveApiBase(baseURI)}${url.slice(API_PREFIX.length)}`
    : url;
}
