import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { GENERATOR_ID, GENERATOR_VERSION, parseHex } from "./gen.ts";

export const VECTORS_PATH = fileURLToPath(
  new URL("../fixtures/generator-vectors.json", import.meta.url),
);

export interface SliceVector {
  readonly source: string | null;
  readonly seed: string;
  readonly offset: number;
  readonly bytes: Uint8Array;
}

export interface ReferenceVectors {
  readonly published: readonly SliceVector[];
  readonly slices: readonly SliceVector[];
}

function decodeBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) {
    throw new Error("vector hex has odd length");
  }
  return parseHex(hex, hex.length / 2, "INVALID_ARGUMENT");
}

function toVector(value: unknown, withSource: boolean): SliceVector {
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record).sort().join(",");
  const expected = withSource ? "hex,offset,seed,source" : "hex,offset,seed";
  if (keys !== expected) {
    throw new Error(`vector has unexpected fields: ${keys}`);
  }
  const { seed, offset, hex, source } = record;
  if (
    typeof seed !== "string" ||
    typeof hex !== "string" ||
    typeof offset !== "number" ||
    !Number.isSafeInteger(offset) ||
    offset < 0
  ) {
    throw new Error("vector has malformed fields");
  }
  return {
    source: withSource ? String(source) : null,
    seed,
    offset,
    bytes: decodeBytes(hex),
  };
}

export function loadVectors(path: string = VECTORS_PATH): ReferenceVectors {
  const raw = JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
  if (raw.version !== GENERATOR_VERSION || raw.generator !== GENERATOR_ID) {
    throw new Error("vector file does not name chacha8-ietf version 1");
  }
  const published = raw.published as unknown[];
  const slices = raw.slices as unknown[];
  return {
    published: published.map((v) => toVector(v, true)),
    slices: slices.map((v) => toVector(v, false)),
  };
}
