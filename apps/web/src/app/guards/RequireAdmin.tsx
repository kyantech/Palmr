import { Outlet } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { ForbiddenPanel } from "../router/StatusPanel";

export const ADMIN_ROLE = "admin";

export function RequireAdmin() {
  const { me } = useBootState();
  return me?.user.role === ADMIN_ROLE ? <Outlet /> : <ForbiddenPanel />;
}
