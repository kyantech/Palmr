#!/usr/bin/env node
import { readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { FALLBACK_LOCALE, NAMESPACES, SUPPORTED_LOCALES } from "../src/app/i18n/catalog.ts";
import {
  DEFAULT_LOCALES_ROOT,
  flattenKeys,
  isPlainObject,
  readNamespace,
} from "./i18n-catalog.mjs";

const localesRoot = resolve(process.argv[2] ?? DEFAULT_LOCALES_ROOT);
const expectedFiles = new Set(NAMESPACES.map((namespace) => `${namespace}.json`));
const problems = [];

function entriesOf(directory) {
  try {
    return readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    console.error(`check-i18n-parity: cannot read ${directory}: ${error.message}`);
    process.exit(2);
  }
}

function loadKeys(locale, namespace) {
  const path = join(localesRoot, locale, `${namespace}.json`);
  let tree;
  try {
    tree = readNamespace(path);
  } catch (error) {
    problems.push(`${locale}/${namespace}.json: unreadable JSON (${error.message})`);
    return null;
  }
  if (!isPlainObject(tree)) {
    problems.push(`${locale}/${namespace}.json: top level must be an object`);
    return null;
  }
  const keys = flattenKeys(tree);
  for (const [key, value] of keys) {
    if (typeof value !== "string") {
      problems.push(`${locale}/${namespace}.json: "${key}" must be a string`);
    }
  }
  return new Set(keys.keys());
}

const localeDirectories = new Set();
for (const entry of entriesOf(localesRoot)) {
  if (!entry.isDirectory() || !SUPPORTED_LOCALES.includes(entry.name)) {
    problems.push(
      `${entry.name}: unexpected entry; only the ${SUPPORTED_LOCALES.length} product locales belong here`,
    );
  } else {
    localeDirectories.add(entry.name);
  }
}

const present = new Map();
for (const locale of SUPPORTED_LOCALES) {
  if (!localeDirectories.has(locale)) {
    problems.push(`${locale}: missing locale directory`);
    continue;
  }
  const files = new Set();
  for (const entry of entriesOf(join(localesRoot, locale))) {
    if (entry.isFile() && expectedFiles.has(entry.name)) {
      files.add(entry.name);
    } else {
      problems.push(`${locale}/${entry.name}: unexpected namespace file`);
    }
  }
  for (const namespace of NAMESPACES) {
    if (!files.has(`${namespace}.json`)) {
      problems.push(`${locale}/${namespace}.json: missing namespace file`);
    } else {
      present.set(`${locale}/${namespace}`, loadKeys(locale, namespace));
    }
  }
}

let canonicalKeyCount = 0;
for (const namespace of NAMESPACES) {
  const canonical = present.get(`${FALLBACK_LOCALE}/${namespace}`);
  if (!canonical) {
    continue;
  }
  canonicalKeyCount += canonical.size;
  for (const locale of SUPPORTED_LOCALES) {
    const keys = present.get(`${locale}/${namespace}`);
    if (locale === FALLBACK_LOCALE || !keys) {
      continue;
    }
    for (const key of canonical) {
      if (!keys.has(key)) {
        problems.push(`${locale}/${namespace}.json: missing key "${key}"`);
      }
    }
    for (const key of keys) {
      if (!canonical.has(key)) {
        problems.push(
          `${locale}/${namespace}.json: orphan key "${key}" (not in ${FALLBACK_LOCALE})`,
        );
      }
    }
  }
}

if (problems.length > 0) {
  for (const problem of problems) {
    console.error(`check-i18n-parity: ${problem}`);
  }
  console.error(
    `check-i18n-parity: ${problems.length} problem(s) (FRONTEND_ARCHITECTURE §12.4, G4)`,
  );
  process.exit(1);
}

console.log(
  `check-i18n-parity: ok (${SUPPORTED_LOCALES.length} locales × ${NAMESPACES.length} namespaces, ` +
    `${canonicalKeyCount} keys, ${relative(process.cwd(), localesRoot) || "."})`,
);
