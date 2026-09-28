import { Navigate, Outlet } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { PATHS } from "../router/paths";

export function RequireEnrolled2FA() {
  const { me } = useBootState();
  return me?.restriction === "mfa_enrollment_required" ? (
    <Navigate to={PATHS.enrollTwoFactor} replace />
  ) : (
    <Outlet />
  );
}
