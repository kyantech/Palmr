export const CSRF_COOKIE = "palmr_csrf";
export const CSRF_HEADER = "X-Palmr-CSRF";

export function readCookie(name: string, cookies: string = document.cookie): string | null {
  for (const pair of cookies.split(";")) {
    const separator = pair.indexOf("=");
    if (separator !== -1 && pair.slice(0, separator).trim() === name) {
      return pair.slice(separator + 1).trim();
    }
  }
  return null;
}
