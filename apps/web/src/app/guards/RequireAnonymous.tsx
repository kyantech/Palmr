import { Navigate, Outlet, useSearchParams } from "react-router";
import { useBootState } from "../bootstrap/bootState";
import { NEXT_PARAM, safeNextPath } from "../router/next";
import { PATHS } from "../router/paths";

export function RequireAnonymous() {
  const { me } = useBootState();
  const [searchParams] = useSearchParams();
  if (me === null) {
    return <Outlet />;
  }
  return <Navigate to={safeNextPath(searchParams.get(NEXT_PARAM)) ?? PATHS.overview} replace />;
}
