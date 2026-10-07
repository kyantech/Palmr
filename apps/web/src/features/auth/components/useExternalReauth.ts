import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { qk } from "../../../shared/api/query-keys";
import type { components } from "../../../shared/api/schema";
import type { ReportedError } from "../../../shared/errors";
import { useStartExternalReauthentication } from "../api/mutations";
import { openExternalReauthChannel } from "../externalReauthChannel";
import { type ExternalReauthLanding, parseExternalReauthMessage } from "../externalReauthMessage";
import { type RecentAuthChallenge, takeRecentAuthReplay } from "../store";

type Me = components["schemas"]["MeResponse"];

export const REAUTH_POPUP_NAME = "palmr-external-reauth";

export const EXTERNAL_REAUTH_WAIT_LIMIT_MS = 600_000;

const POPUP_FEATURES = "popup=yes,width=520,height=720";

export type ExternalReauthPhase = "idle" | "starting" | "waiting" | "verifying";

export type ExternalReauthNotice = "popupBlocked" | "expired";

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
  channel: BroadcastChannel | null;
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

function closeQuietly(popup: Window) {
  try {
    popup.close();
  } catch {
    return;
  }
}

export function useExternalReauth(challenge: RecentAuthChallenge): ExternalReauth {
  const client = useQueryClient();
  const { mutateAsync: requestExternalReauth } = useStartExternalReauthentication();
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
    current.channel?.close();
    closeQuietly(current.popup);
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
    release();
    const popup = window.open("about:blank", REAUTH_POPUP_NAME, POPUP_FEATURES);
    if (popup === null) {
      settle({ notice: "popupBlocked" });
      return;
    }
    busy.current = true;
    setFailure(null);
    setNotice(null);
    setPhase("starting");

    const current: Attempt = {
      popup,
      baseline: recentAuthUntilOf(client.getQueryData<Me | null>(qk.me.current())),
      channel: null,
      stop: () => undefined,
    };
    attempt.current = current;
    const isCurrent = () => attempt.current === current;

    const onMessage = (event: MessageEvent<unknown>) => {
      if (!isCurrent()) {
        return;
      }
      const landing = parseExternalReauthMessage(event.data);
      if (landing === null) {
        return;
      }
      release();
      busy.current = true;
      setPhase("verifying");
      void complete(landing, current.baseline);
    };

    requestExternalReauth().then(
      ({ url, channel }) => {
        if (!isCurrent()) {
          return;
        }
        try {
          current.channel = openExternalReauthChannel(channel);
          current.channel.addEventListener("message", onMessage);
          popup.location.href = url;
        } catch {
          release();
          settle({
            failure: {
              kind: "reported",
              reported: { code: "CLIENT_UNEXPECTED_RESPONSE", requestId: null },
            },
          });
          return;
        }
        const expiry = window.setTimeout(() => {
          if (isCurrent()) {
            release();
            settle({ notice: "expired" });
          }
        }, EXTERNAL_REAUTH_WAIT_LIMIT_MS);
        current.stop = () => {
          window.clearTimeout(expiry);
        };
        busy.current = false;
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
  }, [client, complete, release, requestExternalReauth, settle]);

  return { phase, notice, failure, start };
}
