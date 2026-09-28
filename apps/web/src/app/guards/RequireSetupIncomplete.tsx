import { Navigate, Outlet } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { PATHS } from "../router/paths";

export function RequireSetupIncomplete() {
  const { bootstrap } = useBootState();
  return bootstrap.setupCompleted ? <Navigate to={PATHS.root} replace /> : <Outlet />;
}
