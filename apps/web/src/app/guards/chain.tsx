import type { ComponentType } from "react";
import type { RouteObject } from "react-router";
import { RequireAdmin } from "./RequireAdmin";
import { RequireAuth } from "./RequireAuth";
import { RequireEnrolled2FA } from "./RequireEnrolled2FA";
import { RequireNoPendingPasswordChange } from "./RequireNoPendingPasswordChange";
import { RequireSetup } from "./RequireSetup";

export type Guard = ComponentType;

export const AUTHENTICATED_GUARDS: readonly Guard[] = [
  RequireSetup,
  RequireAuth,
  RequireNoPendingPasswordChange,
  RequireEnrolled2FA,
];

export const ADMIN_GUARDS: readonly Guard[] = [...AUTHENTICATED_GUARDS, RequireAdmin];

export function guardChain(guards: readonly Guard[], children: RouteObject[]): RouteObject[] {
  return guards.reduceRight<RouteObject[]>(
    (inner, Guard) => [{ element: <Guard />, children: inner }],
    children,
  );
}

export function authenticatedRoutes(children: RouteObject[]): RouteObject[] {
  return guardChain(AUTHENTICATED_GUARDS, children);
}

export function adminRoutes(children: RouteObject[]): RouteObject[] {
  return guardChain(ADMIN_GUARDS, children);
}
