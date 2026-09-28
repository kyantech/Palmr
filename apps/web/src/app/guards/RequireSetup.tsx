import { Navigate, Outlet } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { PATHS } from "../router/paths";

export function RequireSetup() {
  const { bootstrap } = useBootState();
  return bootstrap.setupCompleted ? <Outlet /> : <Navigate to={PATHS.setup} replace />;
}
