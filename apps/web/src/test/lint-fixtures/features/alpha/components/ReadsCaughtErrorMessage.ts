export function describeFailure(error: unknown): string {
  return error instanceof Error ? error.message : "";
}
