import { Navigate, Outlet, useLocation } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { loginPathWithNext } from "../router/next";

export function RequireAuth() {
  const { me } = useBootState();
  const { pathname, search } = useLocation();
  return me === null ? <Navigate to={loginPathWithNext(pathname, search)} replace /> : <Outlet />;
}
