import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { hasCallbackParams, readCallbackError, withoutCallbackParams } from "../../features/auth";
import type { ReportedError } from "../../shared/errors";

function same(left: ReportedError | null, right: ReportedError | null): boolean {
  return left?.code === right?.code && left?.requestId === right?.requestId;
}

export function useCallbackError(): ReportedError | null {
  const location = useLocation();
  const { pathname, search } = location;
  const state: unknown = location.state;
  const navigate = useNavigate();
  const [reported, setReported] = useState<ReportedError | null>(() => readCallbackError(search));
  const found = readCallbackError(search);
  if (found !== null && !same(found, reported)) {
    setReported(found);
  }
  const carriesParams = hasCallbackParams(search);
  useEffect(() => {
    if (carriesParams) {
      void navigate({ pathname, search: withoutCallbackParams(search) }, { replace: true, state });
    }
  }, [carriesParams, navigate, pathname, search, state]);
  return reported;
}
