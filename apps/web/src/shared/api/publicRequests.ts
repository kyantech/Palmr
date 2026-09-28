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

export function isPublicRequest({ path }: Pick<ApiRequestDescription, "path">): boolean {
  return (
    PUBLIC_EXACT_PATHS.has(path) || PUBLIC_PATH_PREFIXES.some((prefix) => path.startsWith(prefix))
  );
}
