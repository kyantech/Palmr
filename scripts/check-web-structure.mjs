#!/usr/bin/env node
import { readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PROHIBITED_DIRECTORIES = new Set([
  "components",
  "services",
  "hooks",
  "utils",
  "lib",
  "helpers",
  "common",
  "store",
]);

const defaultSourceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../apps/web/src");
const sourceRoot = resolve(process.argv[2] ?? defaultSourceRoot);

let entries;
try {
  entries = readdirSync(sourceRoot, { withFileTypes: true });
} catch (error) {
  console.error(`check-web-structure: cannot read ${sourceRoot}: ${error.message}`);
  process.exit(2);
}

const violations = entries
  .filter((entry) => entry.isDirectory() && PROHIBITED_DIRECTORIES.has(entry.name.toLowerCase()))
  .map((entry) => relative(process.cwd(), join(sourceRoot, entry.name)))
  .sort();

if (violations.length > 0) {
  for (const path of violations) {
    console.error(
      `check-web-structure: prohibited directory "${path}" (FRONTEND_ARCHITECTURE §2.4). ` +
        "Use features/<x>/ or shared/ instead.",
    );
  }
  process.exit(1);
}

console.log(`check-web-structure: ok (${relative(process.cwd(), sourceRoot) || "."})`);
