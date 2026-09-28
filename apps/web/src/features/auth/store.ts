import { create } from "zustand";

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
