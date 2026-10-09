# Deterministic test-data generator

Introduced in M14-T01. It implements TEST_STRATEGY §5 (P4: no committed giant fixtures) and the G7 gate of §9.1. Large transfers are tested at real sizes with bytes that are produced on demand from a seeded keystream, never stored in git and never held in memory.

| Artefact | Location |
|---|---|
| Manifest (the one integrity contract) | `tests/fixtures/generated-manifest.json` |
| Reference vectors (bytes, not digests) | `tests/fixtures/generator-vectors.json` |
| Rust generator, `GenReader`, `GenHasher`, manifest loader, `gen-verify` | `tests/stress` (crate `palmr-stress`) |
| Node/TypeScript twin, File-like source, manifest loader | `tests/engine-harness` (package `@palmr/engine-harness`) |
| G7 gate script | `scripts/check-generator-manifest.sh` |

Both implementations are test-only. Neither is imported by `apps/server/src` or `apps/web/src`, and `palmr-server` does not depend on `palmr-stress`.

## Generator identity

| Property | Value |
|---|---|
| Name and version | `chacha8-ietf`, version `1` |
| Primitive | ChaCha8, IETF layout: 32-byte key, 12-byte nonce, 32-bit block counter, 64-byte blocks, 8 rounds |
| Key | The 32 bytes of the 64-character hexadecimal `seed`, used verbatim. No hash, KDF, salt or `SeedableRng` |
| Nonce | 12 zero bytes |
| Initial counter | `0` |
| Output | The raw keystream. Nothing is XORed, framed, prefixed or interpreted as words. Words are serialized little-endian on every host |
| Stream identity | `(seed, size)`. The stream is the first `size` bytes of the keystream |
| Seek | Byte offset `n` is block `n / 64`, byte `n % 64`. Positioning is O(1): the cipher counter is set directly and no preceding bytes are generated |
| EOF | Valid positions are `0..size`. Position `size` is EOF: reads return 0 bytes and seeking there is allowed. Seeking past `size` is an error |

The algorithm is frozen. If a harness disagrees with the manifest, the harness is fixed. The manifest changes only through a reviewed commit that also changes `generator` or `version`.

### Capacity: 2^32 - 1 blocks

The IETF counter is 32 bits wide, so one `(seed, nonce)` stream can never exceed 2^32 blocks (256 GiB). The counter is never wrapped, because wrapping would replay keystream bytes.

RustCrypto `chacha20` 0.9.1 (the version already in the dependency graph through `chacha20poly1305`) is stricter than the format: `remaining_blocks()` is `u32::MAX - counter`, so it refuses to generate block `2^32 - 1`. Both harnesses therefore pin the same version-1 limit:

```text
MAX_STREAM_SIZE = (2^32 - 1) * 64 = 274 877 906 880 bytes = 256 GiB - 64 B
```

A declared size above that limit fails with a typed error (`SizeExceedsCapacity` in Rust, `GenError` code `SIZE_EXCEEDS_CAPACITY` in TypeScript) when the stream or the manifest entry is created. The 16 GiB manifest entry and the future 100 GB+ stress sizes fit; a 1 TiB payload does not fit in one version-1 stream. A larger size needs a new, explicitly reviewed generator version, not a silent change here.

## Manifest

```json
{
  "version": 1,
  "generator": "chacha8-ietf",
  "entries": [{ "seed": "<64 lowercase hex>", "size": 67108864, "sha256": "<64 lowercase hex>" }]
}
```

Both loaders reject the manifest as a whole, with a typed error, when any rule fails:

- `version` is not exactly `1`, or `generator` is not exactly `chacha8-ietf`;
- a top-level or entry field is missing or unknown;
- `entries` is empty;
- `seed` or `sha256` is not exactly 64 lowercase hexadecimal characters;
- `size` is not a non-negative integer, or exceeds `MAX_STREAM_SIZE`;
- the same `(seed, size)` appears twice;
- entries are not in ascending `(seed, size)` order.

An entry exists only for a size and seed that a named test uses. The 64 MiB, 1 GiB and 16 GiB entries are the sizes of TEST_STRATEGY §5.2. `seed 2 / 1 MiB` keeps a second key in the PR-tier gate.

Tests read the manifest and compare. They never write it, and there is no mode that updates a digest.

## Reference vectors

`generator-vectors.json` holds raw keystream slices (96 bytes, hex) for three seeds at offsets that cover block boundaries (0, 1, 63, 64, 65), 1 MiB, 1 GiB, 16 GiB and the final 96 bytes of the stream, plus the published ChaCha8 zero-key block. The slices were produced once with the RustCrypto implementation and are committed as constants. Rust and TypeScript each compare their own output with the same constants, so the two implementations are compared byte for byte, not only by digest, and neither regenerates the vectors.

The TypeScript block function is additionally checked against RFC 8439 section 2.3.2 (the same function run with 20 rounds), which validates the quarter round and state layout independently of RustCrypto.

## Using the Rust generator

```rust
use palmr_stress::gen::{hash_async_reader, hash_generated, GenReader, Seed};

let seed = Seed::from_hex("00000000000000000000000000000000000000000000000000000000000000ab")?;
let size = 16 * 1024 * 1024 * 1024;

let expected = hash_generated(&seed, size)?;            // streams; holds one 256 KiB buffer

let body = GenReader::at_offset(&seed, size, resume_offset)?; // tokio::io::AsyncRead
let downloaded = hash_async_reader(&mut server_response_reader).await?;
assert_eq!(downloaded, expected);
```

- `GeneratedStream` is the synchronous core: `new`, `at_offset`, `seek`, `read`, `position`, `remaining`.
- `GenReader` implements `tokio::io::AsyncRead` over it. It serves at most 256 KiB per poll and never allocates per file.
- `GenHasher` is incremental SHA-256 with `update`, `bytes_consumed` and a consuming `finalize`. `hash_generated`, `hash_generated_range`, `hash_stream` and `hash_async_reader` build on it, so the same code hashes generated bytes and bytes that came back from a server.
- A harness compares `sha256(downloaded) == sha256(generated) == manifest[(seed, size)]`.
- `generate_bytes` materializes a bounded slice (at most 256 MiB) for assertions. Whole-file helpers never do.

## Using the Node generator

```ts
import { GeneratedFile } from "./gen.ts";
import { digestOfGeneratedStream, loadManifest } from "./manifest.ts";

const file = new GeneratedFile(seed, 100 * 1024 ** 3);
const part = file.slice(offset, offset + partSize);
const bytes = await part.bytes();
const stream = part.stream();
```

- `GeneratedFile` is a File-like source (`size`, `name`, `type`, `slice`) and `GeneratedBlob` is its lazy slice. Nothing is generated until `bytes()`, `arrayBuffer()` or `stream()` is called.
- `slice(start, end)` accepts only non-negative safe integers. `end` past EOF is clamped to the size, like `Blob.slice`. `start` greater than `end` or past EOF is an error. Negative, fractional, `NaN` and infinite boundaries are errors, not Blob-style relative offsets.
- `bytes()` and `arrayBuffer()` refuse more than 256 MiB. `stream()` yields chunks of at most 256 KiB.
- `GeneratedStream` offers `seek`, `read` and `blocksGenerated` for direct use.

## Commands

All commands run from `v4/` with Node.js 24.

| Purpose | Command |
|---|---|
| G7 (PR tier): manifest entries up to 64 MiB, both languages | `./scripts/check-generator-manifest.sh` |
| Rust generator and manifest tests | `cargo nextest run --locked --profile ci --package palmr-stress` |
| Node generator and manifest tests | `pnpm --filter @palmr/engine-harness test` |
| Full manifest, Rust (includes 1 GiB and 16 GiB) | `cargo run --locked --release --package palmr-stress --bin gen-verify` |
| Full manifest, Node | `pnpm --filter @palmr/engine-harness gen:verify` |
| Subset | `... gen-verify --max-size <bytes>` or `... gen:verify --max-size <bytes>` |
| Digest of one stream | `gen-verify digest --seed <hex> --size <bytes>` or `gen:verify digest --seed <hex> --size <bytes>` |
| Raw bytes at an offset | `gen-verify slice --seed <hex> --size <bytes> --offset <n> --length <n>` |

The full-manifest commands hash every entry completely and print `seed`, `size`, `bytes`, `computed`, `expected` and PASS or FAIL per entry. They exit non-zero on any failure. A 16 GiB entry takes about 12 s in the Rust release build and about 65 s in Node on a developer laptop.

`G7` runs `unit_chacha8_stream_matches_manifest` and `node_chacha8_stream_matches_manifest`. Both load the committed manifest, regenerate every entry of at most 64 MiB, and compare the SHA-256 and the byte count. The script runs both and fails if either fails. It fails on an empty Rust filter (`--no-tests=fail`) and on a Vitest run that does not report the Node test as passed.

CI wiring:

- `.github/workflows/pr.yml`, job `G7 / generator manifest`, blocking, runs the script. The `Web` job also runs format, lint, typecheck and the full Vitest suite of `@palmr/engine-harness`, and the `Rust test` job runs the `palmr-stress` tests as part of the workspace.
- `.github/workflows/nightly.yml`, job `Generator full manifest`, runs both full-manifest commands. This is the full 16 GiB verification required by TEST_STRATEGY §5.7. The `Stress` placeholder job is untouched and still disabled.

The stress tier (`cargo nextest --profile stress`) is for later M14/M15 suites. Nothing in this task runs in that tier, and no 1 GiB or 16 GiB hashing runs in the ordinary nextest suite or on a PR.

## Adding a manifest entry

1. Identify the named stress, chaos or endurance test that needs the size.
2. Choose a fixed, reviewed seed. Do not derive it from a clock or an RNG.
3. Compute the digest with the Rust tool: `gen-verify digest --seed <hex> --size <bytes>`.
4. Compute it again with the Node tool: `pnpm --filter @palmr/engine-harness gen:verify digest --seed <hex> --size <bytes>`. The two digests must be identical.
5. Insert the entry in ascending `(seed, size)` order with the full 64-character digest.
6. Run `./scripts/check-generator-manifest.sh`.
7. Run both full-manifest commands for any entry larger than 64 MiB.
8. Review the diff. The change adds JSON constants only.

A digest is never changed to make a test pass. A mismatch means a harness is wrong.

## Constraints for consumers

- Memory is bounded by the streaming buffer (256 KiB), never by the file size. Do not collect a generated stream into a `Vec<u8>`, `Buffer`, `Blob` or file.
- A resume at offset `n` seeks; it never replays `0..n`.
- Every stream must stay within `MAX_STREAM_SIZE`. Sizes above it require a new generator version.
- Do not use `OsRng`, wall-clock time or process state to produce payload bytes.
- Do not commit generated payloads. Committed fixtures stay under 8 KiB each (`unit_no_generated_payload_fixtures_are_committed`).
