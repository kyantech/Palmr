// @vitest-environment node
import { spawnSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import { NAMESPACES, SUPPORTED_LOCALES } from "./catalog";

const scripts = resolve(import.meta.dirname, "../../../scripts");
const localesRoot = resolve(import.meta.dirname, "locales");
const sandboxes: string[] = [];

function sandbox(): string {
  const root = mkdtempSync(join(tmpdir(), "palmr-i18n-"));
  sandboxes.push(root);
  return root;
}

function copyOfLocales(): string {
  const root = join(sandbox(), "locales");
  cpSync(localesRoot, root, { recursive: true });
  return root;
}

function writeJson(path: string, value: unknown) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

function readJson(path: string): Record<string, unknown> {
  return JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
}

function checkParity(root: string) {
  return spawnSync(process.execPath, [join(scripts, "check-i18n-parity.mjs"), root], {
    encoding: "utf8",
  });
}

afterEach(() => {
  for (const root of sandboxes.splice(0)) {
    rmSync(root, { recursive: true, force: true });
  }
});

test("unit_i18n_key_parity", () => {
  expect(checkParity(localesRoot).status).toBe(0);

  const missing = copyOfLocales();
  const ptErrors = join(missing, "pt-BR", "errors.json");
  const withoutReload = readJson(ptErrors);
  delete (withoutReload.boundary as Record<string, unknown>).reload;
  writeJson(ptErrors, withoutReload);
  const missingResult = checkParity(missing);
  expect(missingResult.status).toBe(1);
  expect(missingResult.stderr).toContain('pt-BR/errors.json: missing key "boundary.reload"');

  const orphan = copyOfLocales();
  writeJson(join(orphan, "de-DE", "common.json"), { onlyInGerman: "Nur auf Deutsch" });
  const orphanResult = checkParity(orphan);
  expect(orphanResult.status).toBe(1);
  expect(orphanResult.stderr).toContain('de-DE/common.json: orphan key "onlyInGerman"');
});

describe("G4 structure", () => {
  test("covers the full catalogue", () => {
    expect(checkParity(localesRoot).stdout).toContain(
      `${String(SUPPORTED_LOCALES.length)} locales × ${String(NAMESPACES.length)} namespaces`,
    );
  });

  test("a missing namespace file fails", () => {
    const root = copyOfLocales();
    unlinkSync(join(root, "ja-JP", "files.json"));

    const result = checkParity(root);

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("ja-JP/files.json: missing namespace file");
  });

  test("an unexpected namespace file fails", () => {
    const root = copyOfLocales();
    writeJson(join(root, "fr-FR", "marketing.json"), {});

    const result = checkParity(root);

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("fr-FR/marketing.json: unexpected namespace file");
  });

  test("a missing or unexpected locale directory fails", () => {
    const root = copyOfLocales();
    rmSync(join(root, "sv-SE"), { recursive: true });
    mkdirSync(join(root, "en-XA"));

    const result = checkParity(root);

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("sv-SE: missing locale directory");
    expect(result.stderr).toContain("en-XA: unexpected entry");
  });

  test("a non-string value or unreadable JSON fails", () => {
    const root = copyOfLocales();
    writeJson(join(root, "ko-KR", "errors.json"), {
      ...readJson(join(root, "ko-KR", "errors.json")),
      staleChunk: { title: 42, description: "x" },
    });
    writeFileSync(join(root, "it-IT", "auth.json"), "{");

    const result = checkParity(root);

    expect(result.status).toBe(1);
    expect(result.stderr).toContain('ko-KR/errors.json: "staleChunk.title" must be a string');
    expect(result.stderr).toContain("it-IT/auth.json: unreadable JSON");
  });

  test("an en-US key missing everywhere else is reported for every other locale", () => {
    const root = copyOfLocales();
    const setup = readJson(join(root, "en-US", "setup.json"));
    writeJson(join(root, "en-US", "setup.json"), { ...setup, parityProbe: "Probe" });

    const result = checkParity(root);

    expect(result.status).toBe(1);
    expect(result.stderr.match(/setup\.json: missing key "parityProbe"/g)).toHaveLength(22);
  });
});

test("the en-XA pseudolocale is generated deterministically outside the product tree", () => {
  const output = sandbox();
  const run = () =>
    spawnSync(process.execPath, [join(scripts, "generate-pseudolocale.mjs"), output], {
      encoding: "utf8",
    });

  expect(run().status).toBe(0);
  const first = readFileSync(join(output, "en-XA", "errors.json"), "utf8");
  expect(run().status).toBe(0);
  expect(readFileSync(join(output, "en-XA", "errors.json"), "utf8")).toBe(first);

  const errors = JSON.parse(first) as {
    boundary: Record<string, string>;
    details: Record<string, string>;
  };
  const english = readJson(join(localesRoot, "en-US", "errors.json")) as {
    boundary: Record<string, string>;
  };
  expect(errors.details.requestId).toContain("{{requestId}}");
  expect(errors.boundary.title).toMatch(/^\[.+~+\]$/);
  expect(errors.boundary.title).not.toBe(english.boundary.title);
  expect(errors.boundary.title?.length).toBeGreaterThan(english.boundary.title?.length ?? 0);
  expect(checkParity(localesRoot).status).toBe(0);
});
