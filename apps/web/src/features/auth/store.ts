import { create } from "zustand";
import type { components } from "../../shared/api/schema";
import type { ErrorDetails } from "../../shared/errors";

export type RecentAuthReplay = () => Promise<unknown>;

export interface RecentAuthChallenge {
  readonly id: number;
  readonly originPath: string;
  readonly requestId: string | null;
  readonly replay: RecentAuthReplay;
}

export interface RecentAuthChallengeInit {
  originPath: string;
  requestId: string | null;
  replay: RecentAuthReplay;
}

interface RecentAuthState {
  challenge: RecentAuthChallenge | null;
}

let nextChallengeId = 1;

export const recentAuthStore = create<RecentAuthState>()(() => ({ challenge: null }));

export function openRecentAuthChallenge(init: RecentAuthChallengeInit): number {
  const id = nextChallengeId++;
  recentAuthStore.setState({ challenge: { id, ...init } });
  return id;
}

export function discardRecentAuthChallenge(id?: number): void {
  const { challenge } = recentAuthStore.getState();
  if (challenge !== null && (id === undefined || challenge.id === id)) {
    recentAuthStore.setState({ challenge: null });
  }
}

export function takeRecentAuthReplay(id: number): RecentAuthReplay | null {
  const { challenge } = recentAuthStore.getState();
  if (challenge?.id !== id) {
    return null;
  }
  recentAuthStore.setState({ challenge: null });
  return challenge.replay;
}

export function useRecentAuthChallenge(): RecentAuthChallenge | null {
  return recentAuthStore((state) => state.challenge);
}

export type SecondFactorMethod = components["schemas"]["SecondFactorMethod"];

const SECOND_FACTOR_METHODS: readonly SecondFactorMethod[] = ["totp", "backup_code"];

export interface MfaChallenge {
  readonly mfaToken: string;
  readonly expiresAt: string;
  readonly methods: readonly SecondFactorMethod[];
  readonly trustedDeviceOffered: boolean;
  readonly deadline: number;
}

interface MfaChallengeState {
  challenge: MfaChallenge | null;
}

export const mfaChallengeStore = create<MfaChallengeState>()(() => ({ challenge: null }));

function isSecondFactorMethod(value: unknown): value is SecondFactorMethod {
  return typeof value === "string" && (SECOND_FACTOR_METHODS as readonly string[]).includes(value);
}

export function beginMfaChallenge(details: ErrorDetails): boolean {
  const { mfaToken, expiresAt, methods, trustedDeviceOffered } = details;
  if (typeof mfaToken !== "string" || mfaToken === "" || typeof expiresAt !== "string") {
    mfaChallengeStore.setState({ challenge: null });
    return false;
  }
  const deadline = Date.parse(expiresAt);
  if (Number.isNaN(deadline)) {
    mfaChallengeStore.setState({ challenge: null });
    return false;
  }
  const offered = Array.isArray(methods) ? methods.filter(isSecondFactorMethod) : [];
  mfaChallengeStore.setState({
    challenge: {
      mfaToken,
      expiresAt,
      methods: offered.length > 0 ? offered : ["totp"],
      trustedDeviceOffered: trustedDeviceOffered === true,
      deadline,
    },
  });
  return true;
}

export function clearMfaChallenge(): void {
  if (mfaChallengeStore.getState().challenge !== null) {
    mfaChallengeStore.setState({ challenge: null });
  }
}

export function isMfaChallengeExpired(challenge: MfaChallenge, now: number = Date.now()): boolean {
  return now >= challenge.deadline;
}

export function useMfaChallenge(): MfaChallenge | null {
  return mfaChallengeStore((state) => state.challenge);
}

export type LoginNotice = "passwordReset" | "twoFactorDisabled" | "challengeExpired";

interface LoginNoticeState {
  notice: LoginNotice | null;
}

export const loginNoticeStore = create<LoginNoticeState>()(() => ({ notice: null }));

export function setLoginNotice(notice: LoginNotice): void {
  loginNoticeStore.setState({ notice });
}

export function clearLoginNotice(): void {
  if (loginNoticeStore.getState().notice !== null) {
    loginNoticeStore.setState({ notice: null });
  }
}

export function useLoginNotice(): LoginNotice | null {
  return loginNoticeStore((state) => state.notice);
}
