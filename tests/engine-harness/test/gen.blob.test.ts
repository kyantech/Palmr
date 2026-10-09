import { describe, expect, test } from "vitest";

import { collect } from "./support.ts";
import {
  GenError,
  GeneratedBlob,
  GeneratedFile,
  MAX_STREAM_SIZE,
  STREAM_CHUNK_SIZE,
  generateBytes,
  toHex,
} from "../gen.ts";
import { loadVectors } from "../vectors.ts";

const KIB = 1024;
const MIB = 1024 * KIB;
const GIB = 1024 * MIB;
const SEED_1 = `${"0".repeat(63)}1`;
const SEED_2 = `${"0".repeat(63)}2`;

async function codeOf(run: () => unknown): Promise<string> {
  try {
    await run();
  } catch (error) {
    if (error instanceof GenError) {
      return error.code;
    }
    throw error;
  }
  return "NO_ERROR";
}

describe("File-like generated source", () => {
  test("slices equal the stream bytes at the same position", async () => {
    const file = new GeneratedFile(SEED_1, 17 * GIB);
    for (const [start, end] of [
      [0, 64],
      [63, 65],
      [MIB, MIB + 128],
      [GIB, GIB + 128],
      [16 * GIB, 16 * GIB + 128],
    ] as const) {
      const bytes = await file.slice(start, end).bytes();
      expect(bytes.length).toBe(end - start);
      expect(toHex(bytes)).toBe(
        toHex(generateBytes(SEED_1, 17 * GIB, start, end - start)),
      );
    }
  });

  test("slice bytes match the committed reference vectors for the same seed", async () => {
    for (const vector of loadVectors().slices) {
      const file = new GeneratedFile(vector.seed, MAX_STREAM_SIZE);
      const bytes = await file
        .slice(vector.offset, vector.offset + vector.bytes.length)
        .bytes();
      expect(toHex(bytes), `${vector.seed}@${vector.offset}`).toBe(
        toHex(vector.bytes),
      );
    }
  });

  test("a multi-gigabyte conceptual file allocates nothing until a bounded slice is read", async () => {
    const file = new GeneratedFile(SEED_2, 100 * GIB);
    expect(file.size).toBe(100 * GIB);
    const slice = file.slice(50 * GIB, 50 * GIB + 4096);
    expect(slice.size).toBe(4096);
    expect((await slice.arrayBuffer()).byteLength).toBe(4096);
    expect(await codeOf(() => file.bytes())).toBe("SLICE_TOO_LARGE");
  });

  test("empty slice, slice at EOF, slice crossing EOF", async () => {
    const file = new GeneratedFile(SEED_1, 1000);
    expect((await file.slice(10, 10).bytes()).length).toBe(0);
    expect((await file.slice(1000, 1000).bytes()).length).toBe(0);
    expect((await file.slice(1000).bytes()).length).toBe(0);
    const crossing = await file.slice(990, 2000).bytes();
    expect(crossing.length).toBe(10);
    expect(toHex(crossing)).toBe(toHex(generateBytes(SEED_1, 1000, 990, 10)));
    expect(file.slice(990, 2000).size).toBe(10);
  });

  test.each([
    ["negative start", -1, 10],
    ["negative end", 0, -1],
    ["fractional start", 0.5, 10],
    ["fractional end", 0, 10.5],
    ["NaN start", Number.NaN, 10],
    ["infinite end", 0, Number.POSITIVE_INFINITY],
    ["unsafe start", 2 ** 53, 2 ** 53 + 2],
  ])("invalid boundary: %s", async (_name, start, end) => {
    const file = new GeneratedFile(SEED_1, 1000);
    expect(await codeOf(() => file.slice(start, end))).toBe("INVALID_RANGE");
  });

  test("start greater than end, or beyond EOF, is rejected", async () => {
    const file = new GeneratedFile(SEED_1, 1000);
    expect(await codeOf(() => file.slice(20, 10))).toBe("INVALID_RANGE");
    expect(await codeOf(() => file.slice(1001, 1002))).toBe("SEEK_PAST_END");
  });

  test("repeated and out-of-order slice calls always return the same bytes", async () => {
    const file = new GeneratedFile(SEED_2, 10 * MIB);
    const first = toHex(await file.slice(5 * MIB, 5 * MIB + 300).bytes());
    await file.slice(0, 64).bytes();
    await file.slice(9 * MIB, 9 * MIB + 7).bytes();
    expect(toHex(await file.slice(5 * MIB, 5 * MIB + 300).bytes())).toBe(first);
    const nested = toHex(
      await file
        .slice(5 * MIB)
        .slice(0, 300)
        .bytes(),
    );
    expect(nested).toBe(first);
  });

  test("slices of slices compose offsets", async () => {
    const file = new GeneratedFile(SEED_1, 4096);
    const inner = file.slice(100, 3000).slice(50, 150);
    expect(inner.size).toBe(100);
    expect(toHex(await inner.bytes())).toBe(
      toHex(generateBytes(SEED_1, 4096, 150, 100)),
    );
  });

  test("stream() yields bounded chunks that concatenate to the slice", async () => {
    const file = new GeneratedFile(SEED_1, 3 * STREAM_CHUNK_SIZE + 11);
    const chunks = await collect(file.stream());
    expect(chunks.length).toBe(4);
    expect(
      Math.max(...chunks.map((chunk) => chunk.length)),
    ).toBeLessThanOrEqual(STREAM_CHUNK_SIZE);
    const joined = Buffer.concat(chunks);
    expect(toHex(joined)).toBe(
      toHex(generateBytes(SEED_1, joined.length, 0, joined.length)),
    );
    expect(() => file.stream(STREAM_CHUNK_SIZE + 1)).toThrow(GenError);
  });

  test("a blob window cannot extend past the stream capacity", () => {
    expect(
      () =>
        new GeneratedBlob(SEED_1, MAX_STREAM_SIZE, MAX_STREAM_SIZE - 10, 20),
    ).toThrow(GenError);
    expect(() => new GeneratedFile(SEED_1, MAX_STREAM_SIZE + 1)).toThrow(
      GenError,
    );
  });
});
