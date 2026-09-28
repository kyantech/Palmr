#!/usr/bin/env node
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { FALLBACK_LOCALE, NAMESPACES } from "../src/app/i18n/catalog.ts";
import { DEFAULT_LOCALES_ROOT, pseudoLocalizeTree, readNamespace } from "./i18n-catalog.mjs";

const PSEUDO_LOCALE = "en-XA";

const [outputRoot, localesRoot = DEFAULT_LOCALES_ROOT] = process.argv.slice(2);
if (!outputRoot) {
  console.error("usage: generate-pseudolocale.mjs <output-dir> [locales-root]");
  process.exit(2);
}

const target = join(resolve(outputRoot), PSEUDO_LOCALE);
mkdirSync(target, { recursive: true });
for (const namespace of NAMESPACES) {
  const source = readNamespace(join(resolve(localesRoot), FALLBACK_LOCALE, `${namespace}.json`));
  writeFileSync(
    join(target, `${namespace}.json`),
    `${JSON.stringify(pseudoLocalizeTree(source), null, 2)}\n`,
  );
}

console.log(`generate-pseudolocale: wrote ${NAMESPACES.length} namespaces to ${target}`);
