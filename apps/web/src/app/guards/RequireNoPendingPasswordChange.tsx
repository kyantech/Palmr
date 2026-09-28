import { Navigate, Outlet } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { PATHS } from "../router/paths";

export function RequireNoPendingPasswordChange() {
  const { me } = useBootState();
  return me?.restriction === "must_change_password" ? (
    <Navigate to={PATHS.forcedPasswordChange} replace />
  ) : (
    <Outlet />
  );
}
