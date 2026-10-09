import { readFileSync } from "node:fs";

import { describe, expect, test } from "vitest";

import {
  G7_MAX_SIZE,
  MANIFEST_PATH,
  ManifestError,
  assertEntryMatches,
  digestOfGeneratedStream,
  loadManifest,
  parseManifest,
  selectEntries,
  validateManifest,
} from "../manifest.ts";

const SEED_1 = `${"0".repeat(63)}1`;
const SEED_2 = `${"0".repeat(63)}2`;
const DIGEST_A = "a".repeat(64);
const DIGEST_B = "b".repeat(64);

function valid(
  entries: unknown[] = [{ seed: SEED_1, size: 64, sha256: DIGEST_A }],
): Record<string, unknown> {
  return { version: 1, generator: "chacha8-ietf", entries };
}

function codeOf(run: () => unknown): string {
  try {
    run();
  } catch (error) {
    if (error instanceof ManifestError) {
      return error.code;
    }
    throw error;
  }
  return "NO_ERROR";
}

test("node_chacha8_stream_matches_manifest", () => {
  const manifest = loadManifest();
  const raw = JSON.parse(readFileSync(MANIFEST_PATH, "utf8")) as {
    entries: { size: number }[];
  };
  const expectedCount = raw.entries.filter(
    (entry) => entry.size <= G7_MAX_SIZE,
  ).length;
  const selected = selectEntries(manifest, G7_MAX_SIZE);

  expect(selected.length).toBeGreaterThan(0);
  expect(selected).toHaveLength(expectedCount);
  expect(new Set(selected.map((entry) => entry.seed)).size).toBeGreaterThan(1);

  for (const entry of selected) {
    const report = assertEntryMatches(entry);
    expect(report.bytes).toBe(entry.size);
    expect(report.computed).toBe(entry.sha256);
  }
});

describe("manifest validation", () => {
  test("the committed manifest validates and is canonical", () => {
    const manifest = loadManifest();
    expect(manifest.version).toBe(1);
    expect(manifest.generator).toBe("chacha8-ietf");
    expect(manifest.entries.length).toBeGreaterThanOrEqual(4);
    expect(readFileSync(MANIFEST_PATH).length).toBeLessThan(8 * 1024);
  });

  test.each([
    [
      "wrong version",
      { ...valid(), version: 2 },
      "MANIFEST_UNSUPPORTED_VERSION",
    ],
    [
      "string version",
      { ...valid(), version: "1" },
      "MANIFEST_UNSUPPORTED_VERSION",
    ],
    [
      "wrong generator",
      { ...valid(), generator: "chacha20-ietf" },
      "MANIFEST_UNSUPPORTED_GENERATOR",
    ],
    ["empty entries", valid([]), "MANIFEST_NO_ENTRIES"],
    [
      "entries not an array",
      { ...valid(), entries: {} },
      "MANIFEST_INVALID_SHAPE",
    ],
    [
      "extra top-level field",
      { ...valid(), extra: 1 },
      "MANIFEST_INVALID_SHAPE",
    ],
    [
      "missing top-level field",
      { version: 1, generator: "chacha8-ietf" },
      "MANIFEST_INVALID_SHAPE",
    ],
    ["entry not an object", valid([null]), "MANIFEST_INVALID_SHAPE"],
    [
      "entry extra field",
      valid([{ seed: SEED_1, size: 1, sha256: DIGEST_A, note: "x" }]),
      "MANIFEST_INVALID_SHAPE",
    ],
    [
      "entry missing digest",
      valid([{ seed: SEED_1, size: 1 }]),
      "MANIFEST_INVALID_SHAPE",
    ],
    [
      "short seed",
      valid([{ seed: "00", size: 1, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SEED",
    ],
    [
      "uppercase seed",
      valid([{ seed: "A".repeat(64), size: 1, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SEED",
    ],
    [
      "non-hex seed",
      valid([{ seed: "g".repeat(64), size: 1, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SEED",
    ],
    [
      "numeric seed",
      valid([{ seed: 1, size: 1, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SEED",
    ],
    [
      "negative size",
      valid([{ seed: SEED_1, size: -1, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SIZE",
    ],
    [
      "fractional size",
      valid([{ seed: SEED_1, size: 1.5, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SIZE",
    ],
    [
      "string size",
      valid([{ seed: SEED_1, size: "64", sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SIZE",
    ],
    [
      "unsafe size",
      valid([{ seed: SEED_1, size: 2 ** 53, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SIZE",
    ],
    [
      "size beyond stream capacity",
      valid([{ seed: SEED_1, size: 2 ** 38, sha256: DIGEST_A }]),
      "MANIFEST_INVALID_SIZE",
    ],
    [
      "truncated digest",
      valid([{ seed: SEED_1, size: 1, sha256: "a".repeat(63) }]),
      "MANIFEST_INVALID_DIGEST",
    ],
    [
      "uppercase digest",
      valid([{ seed: SEED_1, size: 1, sha256: "A".repeat(64) }]),
      "MANIFEST_INVALID_DIGEST",
    ],
    [
      "placeholder digest",
      valid([{ seed: SEED_1, size: 1, sha256: "…" }]),
      "MANIFEST_INVALID_DIGEST",
    ],
    [
      "duplicate seed/size",
      valid([
        { seed: SEED_1, size: 64, sha256: DIGEST_A },
        { seed: SEED_1, size: 64, sha256: DIGEST_B },
      ]),
      "MANIFEST_DUPLICATE_ENTRY",
    ],
    [
      "size out of order",
      valid([
        { seed: SEED_1, size: 128, sha256: DIGEST_A },
        { seed: SEED_1, size: 64, sha256: DIGEST_B },
      ]),
      "MANIFEST_UNSORTED",
    ],
    [
      "seed out of order",
      valid([
        { seed: SEED_2, size: 64, sha256: DIGEST_A },
        { seed: SEED_1, size: 64, sha256: DIGEST_B },
      ]),
      "MANIFEST_UNSORTED",
    ],
  ])("%s is rejected", (_name, manifest, code) => {
    expect(codeOf(() => validateManifest(manifest))).toBe(code);
  });

  test("a stream-capacity-sized entry is accepted", () => {
    const manifest = validateManifest(
      valid([{ seed: SEED_1, size: 274_877_906_880, sha256: DIGEST_A }]),
    );
    expect(manifest.entries[0]?.size).toBe(274_877_906_880);
  });

  test("unparseable JSON is rejected", () => {
    expect(codeOf(() => parseManifest("{"))).toBe("MANIFEST_INVALID_JSON");
    expect(codeOf(() => parseManifest(""))).toBe("MANIFEST_INVALID_JSON");
  });

  test("a missing file is reported, not skipped", () => {
    expect(codeOf(() => loadManifest(`${MANIFEST_PATH}.missing`))).toBe(
      "MANIFEST_UNREADABLE",
    );
  });
});

describe("integrity gate is a hard failure", () => {
  test("a deliberately altered digest fails with a mismatch", () => {
    const [entry] = selectEntries(loadManifest(), G7_MAX_SIZE);
    if (entry === undefined) {
      throw new Error("manifest has no G7 entry");
    }
    const flipped = `${entry.sha256.startsWith("0") ? "1" : "0"}${entry.sha256.slice(1)}`;
    expect(
      codeOf(() => assertEntryMatches({ ...entry, sha256: flipped })),
    ).toBe("MANIFEST_DIGEST_MISMATCH");
  });

  test("a wrong size changes the digest", () => {
    const [entry] = selectEntries(loadManifest(), G7_MAX_SIZE);
    if (entry === undefined) {
      throw new Error("manifest has no G7 entry");
    }
    expect(
      codeOf(() => assertEntryMatches({ ...entry, size: entry.size - 1 })),
    ).toBe("MANIFEST_DIGEST_MISMATCH");
  });

  test("the digest of the empty stream is the SHA-256 of nothing", () => {
    expect(digestOfGeneratedStream(SEED_1, 0)).toEqual({
      bytes: 0,
      sha256:
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    });
  });
});
