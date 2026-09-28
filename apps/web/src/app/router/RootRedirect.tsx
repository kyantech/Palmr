import { Navigate } from "react-router";
import type { BootState } from "../bootstrap/bootState";
import { useBootState } from "../bootstrap/bootState";
import { PATHS } from "./paths";

export function rootDestination({ bootstrap, me }: BootState): string {
  if (!bootstrap.setupCompleted) {
    return PATHS.setup;
  }
  return me === null ? PATHS.login : PATHS.overview;
}

export function RootRedirect() {
  return <Navigate to={rootDestination(useBootState())} replace />;
}
