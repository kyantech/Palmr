import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { qk } from "../../../shared/api/query-keys";
import type { components } from "../../../shared/api/schema";
import type { ReportedError } from "../../../shared/errors";
import { useStartExternalReauthentication } from "../api/mutations";
import { type ExternalReauthLanding, parseExternalReauthMessage } from "../externalReauthMessage";
import { type RecentAuthChallenge, takeRecentAuthReplay } from "../store";

type Me = components["schemas"]["MeResponse"];

export const REAUTH_POPUP_NAME = "palmr-external-reauth";

const POPUP_FEATURES = "popup=yes,width=520,height=720";
const CLOSE_POLL_MS = 500;
const CLOSE_GRACE_MS = 400;

export type ExternalReauthPhase = "idle" | "starting" | "waiting" | "verifying";

export type ExternalReauthNotice = "popupBlocked" | "popupClosed";

export type ExternalReauthFailure =
  | { readonly kind: "api"; readonly error: unknown }
  | { readonly kind: "reported"; readonly reported: ReportedError };

export interface ExternalReauth {
  phase: ExternalReauthPhase;
  notice: ExternalReauthNotice | null;
  failure: ExternalReauthFailure | null;
  start: () => void;
}

interface Attempt {
  readonly popup: Window;
  readonly baseline: number | null;
  stop: () => void;
}

function recentAuthUntilOf(me: Me | null | undefined): number | null {
  const parsed = me == null ? Number.NaN : Date.parse(me.session.recentAuthUntil);
  return Number.isFinite(parsed) ? parsed : null;
}

export function recentAuthWindowOpened(me: Me | null | undefined, baseline: number | null) {
  const next = recentAuthUntilOf(me);
  return next !== null && next > (baseline ?? Date.now());
}

export function useExternalReauth(challenge: RecentAuthChallenge): ExternalReauth {
  const client = useQueryClient();
  const { mutateAsync: requestExternalReauthUrl } = useStartExternalReauthentication();
  const [phase, setPhase] = useState<ExternalReauthPhase>("idle");
  const [notice, setNotice] = useState<ExternalReauthNotice | null>(null);
  const [failure, setFailure] = useState<ExternalReauthFailure | null>(null);
  const attempt = useRef<Attempt | null>(null);
  const busy = useRef(false);

  const release = useCallback(() => {
    const current = attempt.current;
    attempt.current = null;
    if (current === null) {
      return;
    }
    current.stop();
    if (!current.popup.closed) {
      current.popup.close();
    }
  }, []);

  useEffect(
    () => () => {
      release();
    },
    [release],
  );

  const settle = useCallback(
    (next: { notice?: ExternalReauthNotice; failure?: ExternalReauthFailure }) => {
      busy.current = false;
      setPhase("idle");
      setNotice(next.notice ?? null);
      setFailure(next.failure ?? null);
    },
    [],
  );

  const complete = useCallback(
    async (landing: ExternalReauthLanding, baseline: number | null) => {
      if (landing.status === "error") {
        settle({ failure: { kind: "reported", reported: landing.reported } });
        return;
      }
      setPhase("verifying");
      await client.refetchQueries({ queryKey: qk.me.current(), exact: true });
      const me = client.getQueryData<Me | null>(qk.me.current());
      if (!recentAuthWindowOpened(me, baseline)) {
        settle({
          failure: {
            kind: "reported",
            reported: { code: "AUTH_RECENT_AUTH_REQUIRED", requestId: null },
          },
        });
        return;
      }
      const replay = takeRecentAuthReplay(challenge.id);
      if (replay !== null) {
        replay().catch(() => undefined);
      }
    },
    [client, challenge.id, settle],
  );

  const start = useCallback(() => {
    if (busy.current) {
      return;
    }
    const popup = window.open("about:blank", REAUTH_POPUP_NAME, POPUP_FEATURES);
    if (popup === null) {
      setFailure(null);
      setNotice("popupBlocked");
      return;
    }
    busy.current = true;
    setFailure(null);
    setNotice(null);
    setPhase("starting");

    const current: Attempt = {
      popup,
      baseline: recentAuthUntilOf(client.getQueryData<Me | null>(qk.me.current())),
      stop: () => undefined,
    };
    attempt.current = current;
    const isCurrent = () => attempt.current === current;

    const onMessage = (event: MessageEvent<unknown>) => {
      if (!isCurrent() || event.origin !== window.location.origin || event.source !== popup) {
        return;
      }
      const landing = parseExternalReauthMessage(event.data);
      if (landing === null) {
        return;
      }
      release();
      void complete(landing, current.baseline);
    };
    const watcher = window.setInterval(() => {
      if (!popup.closed) {
        return;
      }
      window.clearInterval(watcher);
      window.setTimeout(() => {
        if (isCurrent()) {
          release();
          settle({ notice: "popupClosed" });
        }
      }, CLOSE_GRACE_MS);
    }, CLOSE_POLL_MS);
    window.addEventListener("message", onMessage);
    current.stop = () => {
      window.removeEventListener("message", onMessage);
      window.clearInterval(watcher);
    };

    requestExternalReauthUrl().then(
      (url) => {
        if (!isCurrent()) {
          return;
        }
        try {
          popup.location.href = url;
        } catch {
          release();
          settle({ notice: "popupClosed" });
          return;
        }
        setPhase("waiting");
      },
      (error: unknown) => {
        if (!isCurrent()) {
          return;
        }
        release();
        settle({ failure: { kind: "api", error } });
      },
    );
  }, [client, complete, release, requestExternalReauthUrl, settle]);

  return { phase, notice, failure, start };
}
