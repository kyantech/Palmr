export function nextDelay(attempt: number): number {
  return Math.min(1000 * 2 ** attempt, 60_000);
}
