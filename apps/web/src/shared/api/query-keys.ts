export const qk = {
  bootstrap: () => ["bootstrap"] as const,

  me: {
    all: () => ["me"] as const,
    current: () => ["me", "current"] as const,
  },
} as const;
