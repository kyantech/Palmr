import { useEffect } from "react";
import { Navigate, Outlet, useLocation } from "react-router";
import { clearMfaChallenge, isMfaChallengeExpired, useMfaChallenge } from "../../features/auth";
import { keepNext } from "../router/next";
import { PATHS } from "../router/paths";

export function RequireMfaPending() {
  const challenge = useMfaChallenge();
  const { search } = useLocation();
  const expired = challenge !== null && isMfaChallengeExpired(challenge);
  useEffect(() => {
    if (expired) {
      clearMfaChallenge();
    }
  }, [expired]);
  return challenge === null || expired ? (
    <Navigate to={keepNext(PATHS.login, search)} replace />
  ) : (
    <Outlet />
  );
}
