import { parseArgs } from "node:util";

import {
  MANIFEST_PATH,
  digestOfGeneratedStream,
  loadManifest,
  selectEntries,
  verifyEntry,
} from "./manifest.ts";

const USAGE = `Usage:
  node verify-manifest.ts [--manifest <path>] [--max-size <bytes>]
  node verify-manifest.ts digest --seed <64 hex> --size <bytes>`;

function fail(message: string): never {
  process.stderr.write(`${message}\n${USAGE}\n`);
  process.exit(2);
}

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    manifest: { type: "string", default: MANIFEST_PATH },
    "max-size": { type: "string" },
    seed: { type: "string" },
    size: { type: "string" },
  },
});

function parseBytes(text: string | undefined, name: string): number {
  if (text === undefined || !/^[0-9]+$/.test(text)) {
    return fail(`--${name} must be a non-negative integer number of bytes`);
  }
  return Number(text);
}

if (positionals[0] === "digest") {
  if (positionals.length !== 1 || values.seed === undefined) {
    fail("digest requires --seed and --size");
  }
  const size = parseBytes(values.size, "size");
  const { bytes, sha256 } = digestOfGeneratedStream(values.seed, size);
  process.stdout.write(
    `${JSON.stringify({ seed: values.seed, size, bytes, sha256 })}\n`,
  );
} else {
  if (positionals.length !== 0) {
    fail(`unexpected argument '${positionals[0]}'`);
  }
  const maxSize =
    values["max-size"] === undefined
      ? undefined
      : parseBytes(values["max-size"], "max-size");
  const manifest = loadManifest(values.manifest);
  const entries = selectEntries(manifest, maxSize);
  if (entries.length === 0) {
    fail("no manifest entry matches the selection");
  }
  let failed = 0;
  for (const entry of entries) {
    const started = process.hrtime.bigint();
    const report = verifyEntry(entry);
    const seconds = Number(process.hrtime.bigint() - started) / 1e9;
    if (!report.passed) {
      failed++;
    }
    process.stdout.write(
      `${report.passed ? "PASS" : "FAIL"} seed=${report.seed} size=${report.size} bytes=${report.bytes} computed=${report.computed} expected=${report.expected} seconds=${seconds.toFixed(1)}\n`,
    );
  }
  process.stdout.write(
    `${entries.length - failed}/${entries.length} manifest entries verified\n`,
  );
  process.exit(failed === 0 ? 0 : 1);
}
