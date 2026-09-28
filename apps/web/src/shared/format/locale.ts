export function localeDisplayName(code: string): string {
  try {
    const name = new Intl.DisplayNames([code], {
      type: "language",
      languageDisplay: "standard",
    }).of(code);
    return name ? `${name.charAt(0).toLocaleUpperCase(code)}${name.slice(1)}` : code;
  } catch {
    return code;
  }
}

export interface LocaleOption {
  value: string;
  label: string;
}

export function localeOptions(codes: readonly string[]): LocaleOption[] {
  return codes
    .map((code) => ({ value: code, label: localeDisplayName(code) }))
    .sort((a, b) => a.label.localeCompare(b.label));
}
