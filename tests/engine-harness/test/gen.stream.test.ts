import { createHash } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  BLOCK_SIZE,
  CHACHA8_ROUNDS,
  ChaChaBlockFunction,
  GenError,
  GeneratedStream,
  MAX_STREAM_SIZE,
  STREAM_CHUNK_SIZE,
  generateBytes,
  parseHex,
  parseSeed,
  toHex,
} from "../gen.ts";
import { digestOfGeneratedStream } from "../manifest.ts";
import { loadVectors } from "../vectors.ts";

const KIB = 1024;
const MIB = 1024 * KIB;
const GIB = 1024 * MIB;
const SEED_1 = `${"0".repeat(63)}1`;
const SEED_2 = `${"0".repeat(63)}2`;

function codeOf(run: () => unknown): string {
  try {
    run();
  } catch (error) {
    if (error instanceof GenError) {
      return error.code;
    }
    throw error;
  }
  return "NO_ERROR";
}

function readAll(seed: string, size: number, chunk: number): Uint8Array {
  const stream = new GeneratedStream(seed, size);
  const out = new Uint8Array(size);
  const buffer = new Uint8Array(chunk);
  let at = 0;
  for (;;) {
    const n = stream.read(buffer);
    if (n === 0) {
      return out;
    }
    out.set(buffer.subarray(0, n), at);
    at += n;
  }
}

function xorshift(seed: number): () => number {
  let state = seed >>> 0 || 1;
  return () => {
    state ^= state << 13;
    state >>>= 0;
    state ^= state >>> 17;
    state ^= state << 5;
    state >>>= 0;
    return state;
  };
}

describe("ChaCha core against independent references", () => {
  test("RFC 8439 section 2.3.2 block function (20 rounds) validates the quarter round and state layout", () => {
    const key = Uint8Array.from({ length: 32 }, (_, i) => i);
    const nonce = parseHex("000000090000004a00000000", 12, "INVALID_ARGUMENT");
    const out = new Uint8Array(64);
    new ChaChaBlockFunction(key, nonce, 20).block(1, out, 0);
    expect(toHex(out)).toBe(
      "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e",
    );
  });

  test("published ChaCha8 vectors and RustCrypto-generated vectors are reproduced byte for byte", () => {
    const vectors = loadVectors();
    expect(vectors.published.length).toBeGreaterThan(0);
    expect(vectors.slices.length).toBeGreaterThan(10);
    for (const vector of [...vectors.published, ...vectors.slices]) {
      const actual = generateBytes(
        vector.seed,
        MAX_STREAM_SIZE,
        vector.offset,
        vector.bytes.length,
      );
      expect(toHex(actual), `${vector.seed}@${vector.offset}`).toBe(
        toHex(vector.bytes),
      );
    }
  });

  test("vectors cover block boundaries, large offsets and the stream tail", () => {
    const offsets = new Set(
      loadVectors().slices.map((vector) => vector.offset),
    );
    for (const required of [
      0,
      1,
      63,
      64,
      65,
      MIB,
      GIB,
      16 * GIB,
      MAX_STREAM_SIZE - 96,
    ]) {
      expect(offsets.has(required), `offset ${required}`).toBe(true);
    }
  });

  test("the first keystream block is the zero-nonce counter-0 block", () => {
    const block = new Uint8Array(64);
    new ChaChaBlockFunction(
      parseSeed(SEED_1),
      new Uint8Array(12),
      CHACHA8_ROUNDS,
    ).block(0, block, 0);
    expect(toHex(generateBytes(SEED_1, 64, 0, 64))).toBe(toHex(block));
  });
});

describe("stream identity and chunk independence", () => {
  test.each([0, 1, 63, 64, 65, 127, 128, 129, 4095, 4096, 4097])(
    "size %i is chunk independent",
    (size) => {
      const reference = readAll(SEED_1, size, STREAM_CHUNK_SIZE);
      for (const chunk of [1, 7, 63, 64, 65, 4096, STREAM_CHUNK_SIZE]) {
        expect(toHex(readAll(SEED_1, size, chunk))).toBe(toHex(reference));
      }
    },
  );

  test("irregular chunking produces the same bytes", () => {
    const size = MIB + 37;
    const reference = readAll(SEED_2, size, STREAM_CHUNK_SIZE);
    const next = xorshift(0x5eed);
    const stream = new GeneratedStream(SEED_2, size);
    const out = new Uint8Array(size);
    let at = 0;
    while (at < size) {
      const want = 1 + (next() % 5000);
      const buffer = new Uint8Array(want);
      const n = stream.read(buffer);
      out.set(buffer.subarray(0, n), at);
      at += n;
    }
    expect(Buffer.compare(out, reference)).toBe(0);
  });

  test("same seed and size reproduce, different seeds and a longer size extend", () => {
    const a = readAll(SEED_1, 4097, 1000);
    expect(toHex(readAll(SEED_1, 4097, 777))).toBe(toHex(a));
    expect(toHex(readAll(SEED_2, 4097, 1000))).not.toBe(toHex(a));
    const longer = readAll(SEED_1, 5000, 1000);
    expect(toHex(longer.subarray(0, 4097))).toBe(toHex(a));
  });

  test("different sizes give different full-stream digests", () => {
    expect(digestOfGeneratedStream(SEED_1, 4096).sha256).not.toBe(
      digestOfGeneratedStream(SEED_1, 4097).sha256,
    );
  });

  test("a streamed digest equals the one-shot digest of the materialized bytes", () => {
    const size = 3 * MIB + 5;
    for (const chunk of [1000, 4096, STREAM_CHUNK_SIZE]) {
      const streamed = digestOfGeneratedStream(SEED_2, size, chunk);
      expect(streamed.bytes).toBe(size);
      expect(streamed.sha256).toBe(
        createHash("sha256")
          .update(generateBytes(SEED_2, size, 0, size))
          .digest("hex"),
      );
    }
  });
});

describe("seek semantics", () => {
  test("seek is positioned directly: work depends on the output length, not the offset", () => {
    const vectors = loadVectors().slices.filter(
      (vector) => vector.seed === SEED_1,
    );
    const atSixteenGiB = vectors.find((vector) => vector.offset === 16 * GIB);
    expect(atSixteenGiB).toBeDefined();
    for (const offset of [
      0,
      1,
      63,
      64,
      65,
      MIB,
      64 * MIB,
      GIB,
      16 * GIB,
      MAX_STREAM_SIZE - 96,
    ]) {
      const stream = new GeneratedStream(SEED_1, MAX_STREAM_SIZE);
      stream.seek(offset);
      expect(stream.blocksGenerated).toBe(0);
      const out = new Uint8Array(96);
      expect(stream.read(out)).toBe(96);
      expect(stream.blocksGenerated).toBe(
        Math.ceil(((offset % BLOCK_SIZE) + 96) / BLOCK_SIZE),
      );
      expect(stream.position).toBe(offset + 96);
    }
    const stream = new GeneratedStream(SEED_1, MAX_STREAM_SIZE);
    stream.seek(16 * GIB);
    const out = new Uint8Array(atSixteenGiB?.bytes.length ?? 0);
    stream.read(out);
    expect(toHex(out)).toBe(toHex(atSixteenGiB?.bytes ?? new Uint8Array()));
  });

  test("seeking to an offset equals streaming from zero", () => {
    const size = 100_000;
    const whole = readAll(SEED_1, size, 4096);
    for (const offset of [
      0, 1, 31, 63, 64, 65, 127, 128, 129, 4095, 4096, 4097, 99_999, 100_000,
    ]) {
      const stream = new GeneratedStream(SEED_1, size);
      stream.seek(offset);
      const out = new Uint8Array(size - offset);
      expect(stream.read(out)).toBe(size - offset);
      expect(toHex(out)).toBe(toHex(whole.subarray(offset)));
    }
  });

  test("backward, forward, repeated and current-position seeks are consistent", () => {
    const size = 10_000;
    const whole = readAll(SEED_2, size, 4096);
    const stream = new GeneratedStream(SEED_2, size);
    const grab = (offset: number, length: number): string => {
      stream.seek(offset);
      const out = new Uint8Array(length);
      stream.read(out);
      return toHex(out);
    };
    expect(grab(5000, 100)).toBe(toHex(whole.subarray(5000, 5100)));
    expect(grab(10, 100)).toBe(toHex(whole.subarray(10, 110)));
    expect(grab(9000, 1000)).toBe(toHex(whole.subarray(9000, 10_000)));
    expect(grab(0, 64)).toBe(toHex(whole.subarray(0, 64)));
    stream.seek(stream.position);
    expect(stream.position).toBe(64);
    expect(grab(5000, 100)).toBe(toHex(whole.subarray(5000, 5100)));
    expect(grab(5000, 100)).toBe(toHex(whole.subarray(5000, 5100)));
  });

  test("EOF: seeking to the size is allowed, reading there returns nothing, seeking past it fails", () => {
    const stream = new GeneratedStream(SEED_1, 200);
    stream.seek(200);
    expect(stream.read(new Uint8Array(64))).toBe(0);
    expect(stream.blocksGenerated).toBe(0);
    expect(codeOf(() => stream.seek(201))).toBe("SEEK_PAST_END");
    expect(stream.position).toBe(200);
    stream.seek(190);
    const out = new Uint8Array(64);
    expect(stream.read(out)).toBe(10);
    expect(stream.read(out)).toBe(0);
  });

  test.each([
    -1,
    1.5,
    Number.NaN,
    Number.POSITIVE_INFINITY,
    2 ** 53,
    "1" as unknown as number,
  ])("seek(%s) is rejected", (offset) => {
    const stream = new GeneratedStream(SEED_1, 10);
    expect(codeOf(() => stream.seek(offset))).toBe("INVALID_OFFSET");
    expect(stream.position).toBe(0);
  });
});

describe("capacity and validation", () => {
  test("one stream addresses at most 2^32 - 1 blocks (2^38 - 64 bytes)", () => {
    expect(MAX_STREAM_SIZE).toBe(2 ** 38 - 64);
    expect(new GeneratedStream(SEED_1, MAX_STREAM_SIZE).size).toBe(
      MAX_STREAM_SIZE,
    );
    expect(codeOf(() => new GeneratedStream(SEED_1, MAX_STREAM_SIZE + 1))).toBe(
      "SIZE_EXCEEDS_CAPACITY",
    );
    expect(codeOf(() => new GeneratedStream(SEED_1, 2 ** 38))).toBe(
      "SIZE_EXCEEDS_CAPACITY",
    );
    expect(codeOf(() => new GeneratedStream(SEED_1, 2 ** 40))).toBe(
      "SIZE_EXCEEDS_CAPACITY",
    );
  });

  test("the final byte of the stream is readable and nothing follows it", () => {
    const stream = new GeneratedStream(SEED_1, MAX_STREAM_SIZE);
    stream.seek(MAX_STREAM_SIZE - 1);
    const out = new Uint8Array(64);
    expect(stream.read(out)).toBe(1);
    expect(stream.position).toBe(MAX_STREAM_SIZE);
    expect(stream.read(out)).toBe(0);
  });

  test("the block function refuses a counter outside 32 bits instead of wrapping", () => {
    const cipher = new ChaChaBlockFunction(
      parseSeed(SEED_1),
      new Uint8Array(12),
      CHACHA8_ROUNDS,
    );
    const out = new Uint8Array(64);
    cipher.block(0xffff_ffff, out, 0);
    expect(codeOf(() => cipher.block(2 ** 32, out, 0))).toBe("INVALID_OFFSET");
    expect(codeOf(() => cipher.block(-1, out, 0))).toBe("INVALID_OFFSET");
    expect(codeOf(() => cipher.block(1.5, out, 0))).toBe("INVALID_OFFSET");
  });

  test.each([
    -1,
    1.5,
    Number.NaN,
    Number.POSITIVE_INFINITY,
    2 ** 53,
    "10" as unknown as number,
  ])("size %s is rejected", (size) => {
    expect(codeOf(() => new GeneratedStream(SEED_1, size))).toMatch(
      /^INVALID_SIZE$|^SIZE_EXCEEDS_CAPACITY$/,
    );
  });

  test.each([
    "",
    "00",
    "0".repeat(63),
    "0".repeat(65),
    "g".repeat(64),
    `0x${"0".repeat(62)}`,
    ` ${"0".repeat(63)}`,
    "é".repeat(32),
  ])("malformed seed %j is rejected", (seed) => {
    expect(codeOf(() => new GeneratedStream(seed, 1))).toBe("INVALID_SEED");
  });

  test("generateBytes refuses an oversized single allocation", () => {
    expect(
      codeOf(() => generateBytes(SEED_1, MAX_STREAM_SIZE, 0, 512 * MIB)),
    ).toBe("SLICE_TOO_LARGE");
  });
});
