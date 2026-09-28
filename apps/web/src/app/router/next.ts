import { PATHS } from "./paths";

export const NEXT_PARAM = "next";

const SENTINEL_ORIGIN = "http://palmr.invalid";
function hasUnsafeCharacter(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (character === "\\" || code <= 0x20 || code === 0x7f) {
      return true;
    }
  }
  return false;
}

export function safeNextPath(value: string | null | undefined): string | null {
  if (!value?.startsWith("/") || value.startsWith("//") || hasUnsafeCharacter(value)) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(value, SENTINEL_ORIGIN);
  } catch {
    return null;
  }
  if (url.origin !== SENTINEL_ORIGIN || url.pathname.startsWith("//")) {
    return null;
  }
  return `${url.pathname}${url.search}${url.hash}`;
}

export function loginPathWithNext(pathname: string, search: string): string {
  const next = safeNextPath(`${pathname}${search}`);
  return next === null || next === PATHS.root
    ? PATHS.login
    : `${PATHS.login}?${NEXT_PARAM}=${encodeURIComponent(next)}`;
}
