import { SUPPORTED_LOCALES } from "../i18n/catalog";

export interface LocaleContractMismatch {
  missing: string[];
  unexpected: string[];
}

export class LocaleContractError extends Error {
  readonly mismatch: LocaleContractMismatch;

  constructor(mismatch: LocaleContractMismatch) {
    super(
      `bootstrap.supportedLocales diverges from the frontend catalogue ` +
        `(missing: ${mismatch.missing.join(", ") || "none"}; ` +
        `unexpected: ${mismatch.unexpected.join(", ") || "none"})`,
    );
    this.name = "LocaleContractError";
    this.mismatch = mismatch;
  }
}

export function supportedLocalesMismatch(
  serverLocales: readonly string[],
): LocaleContractMismatch | null {
  const server = new Set(serverLocales);
  const catalogue = new Set<string>(SUPPORTED_LOCALES);
  const missing = SUPPORTED_LOCALES.filter((locale) => !server.has(locale));
  const unexpected = [...server].filter((locale) => !catalogue.has(locale));
  const duplicated = server.size !== serverLocales.length;
  return missing.length === 0 && unexpected.length === 0 && !duplicated
    ? null
    : { missing, unexpected };
}

export function assertSupportedLocales(
  serverLocales: readonly string[],
  strict: boolean = import.meta.env.DEV,
): void {
  const mismatch = supportedLocalesMismatch(serverLocales);
  if (mismatch === null) {
    return;
  }
  const error = new LocaleContractError(mismatch);
  if (strict) {
    throw error;
  }
  console.error(error.message);
}
