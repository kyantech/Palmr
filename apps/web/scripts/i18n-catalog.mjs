import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const DEFAULT_LOCALES_ROOT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../src/app/i18n/locales",
);

export function readNamespace(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

export function isPlainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function flattenKeys(value, prefix = "", keys = new Map()) {
  for (const [key, entry] of Object.entries(value)) {
    const path = prefix === "" ? key : `${prefix}.${key}`;
    if (isPlainObject(entry)) {
      flattenKeys(entry, path, keys);
    } else {
      keys.set(path, entry);
    }
  }
  return keys;
}

const ACCENTED = new Map(
  Object.entries({
    a: "á",
    c: "ç",
    e: "é",
    i: "í",
    n: "ñ",
    o: "ó",
    u: "ú",
    y: "ý",
    A: "Á",
    C: "Ç",
    E: "É",
    I: "Í",
    N: "Ñ",
    O: "Ó",
    U: "Ú",
    Y: "Ý",
  }),
);

const PROTECTED = /(\{\{[^}]*\}\}|\$t\([^)]*\)|<[^>]+>)/;
const EXPANSION = 0.4;

export function pseudoLocalize(text) {
  const accented = text
    .split(PROTECTED)
    .map((part, index) =>
      index % 2 === 1 ? part : [...part].map((char) => ACCENTED.get(char) ?? char).join(""),
    )
    .join("");
  const padding = "~".repeat(Math.ceil(text.length * EXPANSION));
  return `[${accented}${padding}]`;
}

export function pseudoLocalizeTree(value) {
  if (isPlainObject(value)) {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [key, pseudoLocalizeTree(entry)]),
    );
  }
  return typeof value === "string" ? pseudoLocalize(value) : value;
}
