import type { ApiRequestDescription } from "../errors";

const PUBLIC_EXACT_PATHS: ReadonlySet<string> = new Set([
  "/bootstrap",
  "/setup",
  "/auth/login",
  "/auth/email/verify",
]);

const PUBLIC_PATH_PREFIXES: readonly string[] = [
  "/setup/",
  "/auth/login/",
  "/auth/password/",
  "/auth/providers/",
  "/public/",
];

const AUTHENTICATED_PROVIDER_ACTION = /^\/auth\/providers\/[^/]+\/link$/;

export function isPublicRequest({ path }: Pick<ApiRequestDescription, "path">): boolean {
  if (AUTHENTICATED_PROVIDER_ACTION.test(path)) {
    return false;
  }
  return (
    PUBLIC_EXACT_PATHS.has(path) || PUBLIC_PATH_PREFIXES.some((prefix) => path.startsWith(prefix))
  );
}
