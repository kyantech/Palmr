// @vitest-environment node
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, expect, test } from "vitest";

const script = resolve(import.meta.dirname, "../../../../scripts/check-web-structure.mjs");
const sandboxes: string[] = [];

function sourceTree(directories: string[]): string {
  const root = mkdtempSync(join(tmpdir(), "palmr-web-structure-"));
  sandboxes.push(root);
  for (const directory of directories) {
    mkdirSync(join(root, directory), { recursive: true });
  }
  return root;
}

function checkStructure(root: string) {
  return spawnSync(process.execPath, [script, root], { encoding: "utf8" });
}

afterEach(() => {
  for (const root of sandboxes.splice(0)) {
    rmSync(root, { recursive: true, force: true });
  }
});

test("unit_web_structure_accepts_canonical_tree", () => {
  const root = sourceTree([
    "app/layouts",
    "features/files/components",
    "features/files/hooks",
    "shared/hooks",
    "transfer-engine",
  ]);

  const result = checkStructure(root);

  expect(result.status).toBe(0);
});

test("unit_web_structure_rejects_prohibited_directory", () => {
  const root = sourceTree(["app", "utils", "components"]);

  const result = checkStructure(root);

  expect(result.status).toBe(1);
  expect(result.stderr).toMatch(/prohibited directory ".*components"/);
  expect(result.stderr).toMatch(/prohibited directory ".*utils"/);
});
