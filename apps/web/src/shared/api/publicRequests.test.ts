import { expect, test } from "vitest";
import { isPublicRequest } from "./publicRequests";
import { isAuthenticatedQueryKey, qk } from "./query-keys";

test.each([
  "/bootstrap",
  "/setup",
  "/setup/status",
  "/auth/login",
  "/auth/login/totp",
  "/auth/password/forgot",
  "/auth/password/reset",
  "/auth/email/verify",
  "/auth/providers/google/authorize",
  "/auth/providers/{slug}/callback",
  "/public/branding/{asset}",
  "/public/shares/{alias}",
])("unit_public_request_classifier: %s is public", (path) => {
  expect(isPublicRequest({ path })).toBe(true);
});

test.each([
  "/auth/me",
  "/auth/reauthenticate",
  "/auth/logout",
  "/profile",
  "/profile/password",
  "/sessions",
  "/sessions/{id}",
  "/settings/effective",
  "/bootstrapped",
  "/setupx",
  "/auth/loginx",
  "/publicity",
  "/auth/email/verify/extra",
  "/auth/providers/{slug}/link",
  "/auth/providers/google/link",
  "/identity-links",
  "/identity-links/{id}",
])("unit_public_request_classifier: %s is not public", (path) => {
  expect(isPublicRequest({ path })).toBe(false);
});

test("unit_authenticated_query_key_classifier", () => {
  expect(isAuthenticatedQueryKey(qk.bootstrap())).toBe(false);
  expect(isAuthenticatedQueryKey(qk.public.all())).toBe(false);
  expect(isAuthenticatedQueryKey(["public", "share", "abc"])).toBe(false);
  expect(isAuthenticatedQueryKey(qk.me.current())).toBe(false);

  expect(isAuthenticatedQueryKey(qk.me.all())).toBe(true);
  expect(isAuthenticatedQueryKey(["me", "sessions"])).toBe(true);
  expect(isAuthenticatedQueryKey(["me", "current", "extra"])).toBe(true);
  expect(isAuthenticatedQueryKey(["files", "list", null, {}])).toBe(true);
  expect(isAuthenticatedQueryKey(["admin", "users"])).toBe(true);
});
