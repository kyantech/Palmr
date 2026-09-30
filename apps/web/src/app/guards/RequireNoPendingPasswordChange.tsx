import { Navigate, Outlet, useLocation } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { lockPathWithNext } from "../router/next";
import { PATHS } from "../router/paths";

export function RequireNoPendingPasswordChange() {
  const { me } = useBootState();
  const { pathname, search } = useLocation();
  return me?.restriction === "must_change_password" ? (
    <Navigate to={lockPathWithNext(PATHS.forcedPasswordChange, pathname, search)} replace />
  ) : (
    <Outlet />
  );
}
