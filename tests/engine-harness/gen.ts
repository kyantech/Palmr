export const GENERATOR_ID = "chacha8-ietf";
export const GENERATOR_VERSION = 1;
export const SEED_BYTES = 32;
export const NONCE_BYTES = 12;
export const BLOCK_SIZE = 64;
export const CHACHA8_ROUNDS = 8;
export const MAX_BLOCKS = 0xffff_ffff;
export const MAX_STREAM_SIZE = MAX_BLOCKS * BLOCK_SIZE;
export const STREAM_CHUNK_SIZE = 256 * 1024;
export const MAX_MATERIALIZED_BYTES = 256 * 1024 * 1024;

export type GenErrorCode =
  | "INVALID_SEED"
  | "INVALID_SIZE"
  | "SIZE_EXCEEDS_CAPACITY"
  | "INVALID_OFFSET"
  | "SEEK_PAST_END"
  | "INVALID_RANGE"
  | "SLICE_TOO_LARGE"
  | "INVALID_ARGUMENT";

export class GenError extends Error {
  readonly code: GenErrorCode;

  constructor(code: GenErrorCode, message: string) {
    super(`${code}: ${message}`);
    this.name = "GenError";
    this.code = code;
  }
}

const SIGMA = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574] as const;

function rotl(value: number, bits: number): number {
  return (value << bits) | (value >>> (32 - bits));
}

function quarterRound(
  x: Int32Array,
  a: number,
  b: number,
  c: number,
  d: number,
): void {
  x[a] = ((x[a] as number) + (x[b] as number)) | 0;
  x[d] = rotl((x[d] as number) ^ (x[a] as number), 16);
  x[c] = ((x[c] as number) + (x[d] as number)) | 0;
  x[b] = rotl((x[b] as number) ^ (x[c] as number), 12);
  x[a] = ((x[a] as number) + (x[b] as number)) | 0;
  x[d] = rotl((x[d] as number) ^ (x[a] as number), 8);
  x[c] = ((x[c] as number) + (x[d] as number)) | 0;
  x[b] = rotl((x[b] as number) ^ (x[c] as number), 7);
}

function readWordLE(bytes: Uint8Array, offset: number): number {
  return (
    ((bytes[offset] as number) |
      ((bytes[offset + 1] as number) << 8) |
      ((bytes[offset + 2] as number) << 16) |
      ((bytes[offset + 3] as number) << 24)) >>>
    0
  );
}

export class ChaChaBlockFunction {
  readonly rounds: number;
  readonly #state = new Uint32Array(16);
  readonly #work = new Int32Array(16);

  constructor(key: Uint8Array, nonce: Uint8Array, rounds: number) {
    if (key.length !== SEED_BYTES) {
      throw new GenError("INVALID_SEED", `key must be ${SEED_BYTES} bytes`);
    }
    if (nonce.length !== NONCE_BYTES) {
      throw new GenError(
        "INVALID_ARGUMENT",
        `nonce must be ${NONCE_BYTES} bytes`,
      );
    }
    if (!Number.isInteger(rounds) || rounds < 2 || rounds % 2 !== 0) {
      throw new GenError(
        "INVALID_ARGUMENT",
        "rounds must be a positive even integer",
      );
    }
    this.rounds = rounds;
    for (let i = 0; i < 4; i++) {
      this.#state[i] = SIGMA[i] as number;
    }
    for (let i = 0; i < 8; i++) {
      this.#state[4 + i] = readWordLE(key, i * 4);
    }
    for (let i = 0; i < 3; i++) {
      this.#state[13 + i] = readWordLE(nonce, i * 4);
    }
  }

  block(counter: number, out: Uint8Array, outOffset: number): void {
    if (!Number.isInteger(counter) || counter < 0 || counter > 0xffff_ffff) {
      throw new GenError(
        "INVALID_OFFSET",
        "block counter outside the 32-bit range",
      );
    }
    if (outOffset < 0 || outOffset + BLOCK_SIZE > out.length) {
      throw new GenError(
        "INVALID_ARGUMENT",
        "output window is smaller than one block",
      );
    }
    const state = this.#state;
    const x = this.#work;
    state[12] = counter;
    for (let i = 0; i < 16; i++) {
      x[i] = state[i] as number;
    }
    for (let round = 0; round < this.rounds; round += 2) {
      quarterRound(x, 0, 4, 8, 12);
      quarterRound(x, 1, 5, 9, 13);
      quarterRound(x, 2, 6, 10, 14);
      quarterRound(x, 3, 7, 11, 15);
      quarterRound(x, 0, 5, 10, 15);
      quarterRound(x, 1, 6, 11, 12);
      quarterRound(x, 2, 7, 8, 13);
      quarterRound(x, 3, 4, 9, 14);
    }
    for (let i = 0; i < 16; i++) {
      const word = ((x[i] as number) + (state[i] as number)) >>> 0;
      const at = outOffset + i * 4;
      out[at] = word & 0xff;
      out[at + 1] = (word >>> 8) & 0xff;
      out[at + 2] = (word >>> 16) & 0xff;
      out[at + 3] = (word >>> 24) & 0xff;
    }
  }
}

export function parseHex(
  hex: string,
  byteLength: number,
  code: GenErrorCode,
): Uint8Array {
  if (
    typeof hex !== "string" ||
    hex.length !== byteLength * 2 ||
    !/^[0-9a-fA-F]+$/.test(hex)
  ) {
    throw new GenError(
      code,
      `expected exactly ${byteLength * 2} hexadecimal characters`,
    );
  }
  const out = new Uint8Array(byteLength);
  for (let i = 0; i < byteLength; i++) {
    out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

export function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) {
    out += byte.toString(16).padStart(2, "0");
  }
  return out;
}

export function parseSeed(seed: string): Uint8Array {
  return parseHex(seed, SEED_BYTES, "INVALID_SEED");
}

export function assertNonNegativeSafeInteger(
  value: unknown,
  what: string,
  code: GenErrorCode,
): asserts value is number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new GenError(code, `${what} must be a non-negative safe integer`);
  }
}

export function assertStreamSize(size: unknown): asserts size is number {
  assertNonNegativeSafeInteger(size, "size", "INVALID_SIZE");
  if (size > MAX_STREAM_SIZE) {
    throw new GenError(
      "SIZE_EXCEEDS_CAPACITY",
      `size ${size} exceeds the ${MAX_STREAM_SIZE}-byte capacity of one stream`,
    );
  }
}

const ZERO_NONCE = new Uint8Array(NONCE_BYTES);

export class GeneratedStream {
  readonly seed: string;
  readonly size: number;
  readonly #cipher: ChaChaBlockFunction;
  readonly #scratch = new Uint8Array(BLOCK_SIZE);
  #position = 0;
  #blocksGenerated = 0;

  constructor(seed: string, size: number) {
    this.#cipher = new ChaChaBlockFunction(
      parseSeed(seed),
      ZERO_NONCE,
      CHACHA8_ROUNDS,
    );
    assertStreamSize(size);
    this.seed = seed;
    this.size = size;
  }

  get position(): number {
    return this.#position;
  }

  get blocksGenerated(): number {
    return this.#blocksGenerated;
  }

  get remaining(): number {
    return this.size - this.#position;
  }

  seek(offset: number): void {
    assertNonNegativeSafeInteger(offset, "offset", "INVALID_OFFSET");
    if (offset > this.size) {
      throw new GenError(
        "SEEK_PAST_END",
        `offset ${offset} is past the end of the ${this.size}-byte stream`,
      );
    }
    this.#position = offset;
  }

  read(out: Uint8Array): number {
    const wanted = Math.min(out.length, this.size - this.#position);
    let written = 0;
    while (written < wanted) {
      const block = Math.floor(this.#position / BLOCK_SIZE);
      const intra = this.#position % BLOCK_SIZE;
      const take = Math.min(BLOCK_SIZE - intra, wanted - written);
      if (intra === 0 && take === BLOCK_SIZE) {
        this.#cipher.block(block, out, written);
      } else {
        this.#cipher.block(block, this.#scratch, 0);
        out.set(this.#scratch.subarray(intra, intra + take), written);
      }
      this.#blocksGenerated++;
      this.#position += take;
      written += take;
    }
    return written;
  }
}

export function generateBytes(
  seed: string,
  size: number,
  offset: number,
  length: number,
): Uint8Array {
  assertNonNegativeSafeInteger(length, "length", "INVALID_ARGUMENT");
  if (length > MAX_MATERIALIZED_BYTES) {
    throw new GenError(
      "SLICE_TOO_LARGE",
      `${length} bytes exceeds the ${MAX_MATERIALIZED_BYTES}-byte materialization ceiling`,
    );
  }
  const stream = new GeneratedStream(seed, size);
  stream.seek(offset);
  const out = new Uint8Array(Math.min(length, stream.remaining));
  stream.read(out);
  return out;
}

function normalizeBoundary(
  value: number | undefined,
  fallback: number,
  what: string,
): number {
  if (value === undefined) {
    return fallback;
  }
  assertNonNegativeSafeInteger(value, what, "INVALID_RANGE");
  return value;
}

export class GeneratedBlob {
  readonly seed: string;
  readonly size: number;
  readonly type: string;
  readonly #origin: number;
  readonly #streamSize: number;

  constructor(
    seed: string,
    streamSize: number,
    origin: number,
    size: number,
    type = "",
  ) {
    parseSeed(seed);
    assertStreamSize(streamSize);
    assertNonNegativeSafeInteger(origin, "origin", "INVALID_OFFSET");
    assertNonNegativeSafeInteger(size, "size", "INVALID_SIZE");
    if (origin + size > streamSize) {
      throw new GenError(
        "INVALID_RANGE",
        "blob window extends past the end of the stream",
      );
    }
    this.seed = seed;
    this.size = size;
    this.type = type;
    this.#origin = origin;
    this.#streamSize = streamSize;
  }

  slice(start?: number, end?: number, contentType?: string): GeneratedBlob {
    const from = normalizeBoundary(start, 0, "start");
    const to = Math.min(normalizeBoundary(end, this.size, "end"), this.size);
    if (from > this.size) {
      throw new GenError(
        "SEEK_PAST_END",
        `start ${from} is past the end of the ${this.size}-byte blob`,
      );
    }
    if (from > to) {
      throw new GenError(
        "INVALID_RANGE",
        `start ${from} is greater than end ${to}`,
      );
    }
    return new GeneratedBlob(
      this.seed,
      this.#streamSize,
      this.#origin + from,
      to - from,
      contentType ?? this.type,
    );
  }

  async bytes(): Promise<Uint8Array<ArrayBuffer>> {
    if (this.size > MAX_MATERIALIZED_BYTES) {
      throw new GenError(
        "SLICE_TOO_LARGE",
        `${this.size} bytes exceeds the ${MAX_MATERIALIZED_BYTES}-byte materialization ceiling; use stream()`,
      );
    }
    const out = new Uint8Array(this.size);
    const stream = new GeneratedStream(this.seed, this.#streamSize);
    stream.seek(this.#origin);
    stream.read(out);
    return out;
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    return (await this.bytes()).buffer;
  }

  stream(
    chunkSize = STREAM_CHUNK_SIZE,
  ): ReadableStream<Uint8Array<ArrayBuffer>> {
    if (
      !Number.isSafeInteger(chunkSize) ||
      chunkSize <= 0 ||
      chunkSize > STREAM_CHUNK_SIZE
    ) {
      throw new GenError(
        "INVALID_ARGUMENT",
        `chunkSize must be an integer in 1..=${STREAM_CHUNK_SIZE}`,
      );
    }
    const generator = new GeneratedStream(this.seed, this.#streamSize);
    generator.seek(this.#origin);
    let remaining = this.size;
    return new ReadableStream<Uint8Array<ArrayBuffer>>({
      pull(controller) {
        if (remaining === 0) {
          controller.close();
          return;
        }
        const chunk = new Uint8Array(Math.min(chunkSize, remaining));
        generator.read(chunk);
        remaining -= chunk.length;
        controller.enqueue(chunk);
      },
    });
  }
}

export class GeneratedFile extends GeneratedBlob {
  readonly name: string;
  readonly lastModified: number;

  constructor(
    seed: string,
    size: number,
    name = "generated.bin",
    type = "application/octet-stream",
  ) {
    super(seed, size, 0, size, type);
    this.name = name;
    this.lastModified = 0;
  }
}
