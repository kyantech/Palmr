import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import {
  GENERATOR_ID,
  GENERATOR_VERSION,
  GeneratedStream,
  MAX_STREAM_SIZE,
  STREAM_CHUNK_SIZE,
} from "./gen.ts";

export const MANIFEST_PATH = fileURLToPath(
  new URL("../fixtures/generated-manifest.json", import.meta.url),
);
export const G7_MAX_SIZE = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES = 64 * 1024;

export type ManifestErrorCode =
  | "MANIFEST_UNREADABLE"
  | "MANIFEST_INVALID_JSON"
  | "MANIFEST_INVALID_SHAPE"
  | "MANIFEST_UNSUPPORTED_VERSION"
  | "MANIFEST_UNSUPPORTED_GENERATOR"
  | "MANIFEST_NO_ENTRIES"
  | "MANIFEST_INVALID_SEED"
  | "MANIFEST_INVALID_SIZE"
  | "MANIFEST_INVALID_DIGEST"
  | "MANIFEST_DUPLICATE_ENTRY"
  | "MANIFEST_UNSORTED"
  | "MANIFEST_DIGEST_MISMATCH";

export class ManifestError extends Error {
  readonly code: ManifestErrorCode;

  constructor(code: ManifestErrorCode, message: string) {
    super(`${code}: ${message}`);
    this.name = "ManifestError";
    this.code = code;
  }
}

export interface ManifestEntry {
  readonly seed: string;
  readonly size: number;
  readonly sha256: string;
}

export interface Manifest {
  readonly version: 1;
  readonly generator: "chacha8-ietf";
  readonly entries: readonly ManifestEntry[];
}

export interface EntryReport {
  readonly seed: string;
  readonly size: number;
  readonly bytes: number;
  readonly computed: string;
  readonly expected: string;
  readonly passed: boolean;
}

const LOWERCASE_HEX_64 = /^[0-9a-f]{64}$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertKeys(
  record: Record<string, unknown>,
  allowed: readonly string[],
  what: string,
): void {
  for (const key of Object.keys(record)) {
    if (!allowed.includes(key)) {
      throw new ManifestError(
        "MANIFEST_INVALID_SHAPE",
        `${what} has unknown field '${key}'`,
      );
    }
  }
  for (const key of allowed) {
    if (!(key in record)) {
      throw new ManifestError(
        "MANIFEST_INVALID_SHAPE",
        `${what} is missing field '${key}'`,
      );
    }
  }
}

function validateEntry(value: unknown, index: number): ManifestEntry {
  const what = `entries[${index}]`;
  if (!isRecord(value)) {
    throw new ManifestError(
      "MANIFEST_INVALID_SHAPE",
      `${what} is not an object`,
    );
  }
  assertKeys(value, ["seed", "size", "sha256"], what);
  const { seed, size, sha256 } = value;
  if (typeof seed !== "string" || !LOWERCASE_HEX_64.test(seed)) {
    throw new ManifestError(
      "MANIFEST_INVALID_SEED",
      `${what}.seed must be 64 lowercase hexadecimal characters`,
    );
  }
  if (typeof size !== "number" || !Number.isSafeInteger(size) || size < 0) {
    throw new ManifestError(
      "MANIFEST_INVALID_SIZE",
      `${what}.size must be a non-negative safe integer`,
    );
  }
  if (size > MAX_STREAM_SIZE) {
    throw new ManifestError(
      "MANIFEST_INVALID_SIZE",
      `${what}.size exceeds the ${MAX_STREAM_SIZE}-byte stream capacity`,
    );
  }
  if (typeof sha256 !== "string" || !LOWERCASE_HEX_64.test(sha256)) {
    throw new ManifestError(
      "MANIFEST_INVALID_DIGEST",
      `${what}.sha256 must be 64 lowercase hexadecimal characters`,
    );
  }
  return { seed, size, sha256 };
}

export function validateManifest(value: unknown): Manifest {
  if (!isRecord(value)) {
    throw new ManifestError(
      "MANIFEST_INVALID_SHAPE",
      "manifest is not an object",
    );
  }
  assertKeys(value, ["version", "generator", "entries"], "manifest");
  if (value.version !== GENERATOR_VERSION) {
    throw new ManifestError(
      "MANIFEST_UNSUPPORTED_VERSION",
      `version must be ${GENERATOR_VERSION}`,
    );
  }
  if (value.generator !== GENERATOR_ID) {
    throw new ManifestError(
      "MANIFEST_UNSUPPORTED_GENERATOR",
      `generator must be '${GENERATOR_ID}'`,
    );
  }
  if (!Array.isArray(value.entries)) {
    throw new ManifestError(
      "MANIFEST_INVALID_SHAPE",
      "entries is not an array",
    );
  }
  if (value.entries.length === 0) {
    throw new ManifestError("MANIFEST_NO_ENTRIES", "entries must not be empty");
  }
  const entries = value.entries.map((entry: unknown, index: number) =>
    validateEntry(entry, index),
  );
  const seen = new Set<string>();
  entries.forEach((entry, index) => {
    const key = `${entry.seed}/${entry.size}`;
    if (seen.has(key)) {
      throw new ManifestError(
        "MANIFEST_DUPLICATE_ENTRY",
        `entries[${index}] repeats seed/size ${key}`,
      );
    }
    seen.add(key);
    const previous = entries[index - 1];
    if (
      previous !== undefined &&
      (previous.seed > entry.seed ||
        (previous.seed === entry.seed && previous.size > entry.size))
    ) {
      throw new ManifestError(
        "MANIFEST_UNSORTED",
        `entries[${index}] is not in ascending (seed, size) order`,
      );
    }
  });
  return { version: GENERATOR_VERSION, generator: GENERATOR_ID, entries };
}

export function parseManifest(json: string): Manifest {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch (error) {
    throw new ManifestError(
      "MANIFEST_INVALID_JSON",
      error instanceof Error ? error.message : "unparseable JSON",
    );
  }
  return validateManifest(value);
}

export function loadManifest(path: string = MANIFEST_PATH): Manifest {
  let text: string;
  try {
    const bytes = readFileSync(path);
    if (bytes.length > MAX_MANIFEST_BYTES) {
      throw new ManifestError(
        "MANIFEST_UNREADABLE",
        `${path} exceeds ${MAX_MANIFEST_BYTES} bytes`,
      );
    }
    text = bytes.toString("utf8");
  } catch (error) {
    if (error instanceof ManifestError) {
      throw error;
    }
    throw new ManifestError(
      "MANIFEST_UNREADABLE",
      `${path}: ${error instanceof Error ? error.message : "unreadable"}`,
    );
  }
  return parseManifest(text);
}

export interface StreamDigest {
  readonly bytes: number;
  readonly sha256: string;
}

export function digestOfGeneratedStream(
  seed: string,
  size: number,
  chunkSize: number = STREAM_CHUNK_SIZE,
): StreamDigest {
  const stream = new GeneratedStream(seed, size);
  const hash = createHash("sha256");
  const buffer = new Uint8Array(chunkSize);
  let bytes = 0;
  for (;;) {
    const n = stream.read(buffer);
    if (n === 0) {
      break;
    }
    hash.update(buffer.subarray(0, n));
    bytes += n;
  }
  return { bytes, sha256: hash.digest("hex") };
}

export function verifyEntry(entry: ManifestEntry): EntryReport {
  const { bytes, sha256 } = digestOfGeneratedStream(entry.seed, entry.size);
  return {
    seed: entry.seed,
    size: entry.size,
    bytes,
    computed: sha256,
    expected: entry.sha256,
    passed: bytes === entry.size && sha256 === entry.sha256,
  };
}

export function assertEntryMatches(entry: ManifestEntry): EntryReport {
  const report = verifyEntry(entry);
  if (!report.passed) {
    throw new ManifestError(
      "MANIFEST_DIGEST_MISMATCH",
      `seed ${entry.seed} size ${entry.size}: generated ${report.bytes} bytes with sha256 ${report.computed}, manifest expects ${report.expected}`,
    );
  }
  return report;
}

export function selectEntries(
  manifest: Manifest,
  maxSize?: number,
): ManifestEntry[] {
  return manifest.entries.filter(
    (entry) => maxSize === undefined || entry.size <= maxSize,
  );
}
