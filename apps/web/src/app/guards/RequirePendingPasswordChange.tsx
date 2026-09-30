import { Navigate, Outlet, useLocation } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { restrictionDestination } from "../router/next";

export function RequirePendingPasswordChange() {
  const { me } = useBootState();
  const { search } = useLocation();
  const restriction = me?.restriction ?? null;
  return restriction === "must_change_password" ? (
    <Outlet />
  ) : (
    <Navigate to={restrictionDestination(restriction, search)} replace />
  );
}
