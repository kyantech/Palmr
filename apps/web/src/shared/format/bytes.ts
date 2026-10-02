const UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"] as const;

export type ByteUnit = (typeof UNITS)[number];

export const BYTE_UNIT_FACTORS: Readonly<Record<ByteUnit, number>> = {
  B: 1,
  KiB: 1024,
  MiB: 1024 ** 2,
  GiB: 1024 ** 3,
  TiB: 1024 ** 4,
  PiB: 1024 ** 5,
};

export function formatBytes(bytes: number, locale: string): string {
  const magnitude = Math.abs(bytes);
  let unit: ByteUnit = "B";
  for (const candidate of UNITS) {
    if (magnitude >= BYTE_UNIT_FACTORS[candidate] || candidate === "B") {
      unit = candidate;
    }
  }
  const value = bytes / BYTE_UNIT_FACTORS[unit];
  const digits = unit === "B" || Number.isInteger(value) || Math.abs(value) >= 100 ? 0 : 1;
  const formatted = new Intl.NumberFormat(locale, {
    maximumFractionDigits: digits,
    minimumFractionDigits: 0,
  }).format(value);
  return `${formatted} ${unit}`;
}

export interface SplitBytes {
  amount: number;
  unit: ByteUnit;
}

export function splitBytes(bytes: number): SplitBytes {
  if (bytes === 0) {
    return { amount: 0, unit: "GiB" };
  }
  for (const unit of [...UNITS].reverse()) {
    const factor = BYTE_UNIT_FACTORS[unit];
    if (bytes % factor === 0) {
      return { amount: bytes / factor, unit };
    }
  }
  return { amount: bytes, unit: "B" };
}

export const MAX_SAFE_BYTES = Number.MAX_SAFE_INTEGER;

export function toBytes(amount: number, unit: ByteUnit): number {
  return Math.round(amount * BYTE_UNIT_FACTORS[unit]);
}
