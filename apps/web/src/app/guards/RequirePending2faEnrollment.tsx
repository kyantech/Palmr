import { Navigate, Outlet, useLocation } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { restrictionDestination } from "../router/next";

export function RequirePending2faEnrollment() {
  const { me } = useBootState();
  const { search } = useLocation();
  const restriction = me?.restriction ?? null;
  return restriction === "mfa_enrollment_required" ? (
    <Outlet />
  ) : (
    <Navigate to={restrictionDestination(restriction, search)} replace />
  );
}
