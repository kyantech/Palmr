import { Navigate, Outlet, useLocation } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { lockPathWithNext } from "../router/next";
import { PATHS } from "../router/paths";

export function RequireEnrolled2FA() {
  const { me } = useBootState();
  const { pathname, search } = useLocation();
  return me?.restriction === "mfa_enrollment_required" ? (
    <Navigate to={lockPathWithNext(PATHS.enrollTwoFactor, pathname, search)} replace />
  ) : (
    <Outlet />
  );
}
