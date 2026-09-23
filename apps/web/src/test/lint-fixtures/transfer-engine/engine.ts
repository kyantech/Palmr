import { nextDelay } from "./backoff";

export function createEngine() {
  return { queued: nextDelay(0) };
}
