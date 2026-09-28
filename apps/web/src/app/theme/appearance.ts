import { theme, type ThemeConfig } from "antd";

export const THEME_PREFERENCES = ["light", "dark", "system"] as const;

export type ThemePreference = (typeof THEME_PREFERENCES)[number];

export type ResolvedThemeMode = Exclude<ThemePreference, "system">;

export const ACCENT_PRESETS = {
  default: null,
  blue: "#2563eb",
  violet: "#7c3aed",
  emerald: "#047857",
  amber: "#b45309",
  rose: "#e11d48",
  slate: "#475569",
} as const satisfies Readonly<Record<string, `#${string}` | null>>;

export type AccentKey = keyof typeof ACCENT_PRESETS;

export const PALMR_PRIMARY_COLOR = "#1668dc";

const HEX_COLOR = /^#[0-9a-fA-F]{6}$/;

export interface Appearance {
  preference: ThemePreference;
  accent: AccentKey;
  primaryColor: string | null | undefined;
}

export const DEFAULT_APPEARANCE: Appearance = {
  preference: "system",
  accent: "default",
  primaryColor: undefined,
};

export function safePrimaryColor(value: unknown): string {
  return typeof value === "string" && HEX_COLOR.test(value) ? value : PALMR_PRIMARY_COLOR;
}

export function toThemePreference(value: unknown): ThemePreference {
  return THEME_PREFERENCES.find((preference) => preference === value) ?? "system";
}

export function toAccentKey(value: unknown): AccentKey {
  return typeof value === "string" && Object.hasOwn(ACCENT_PRESETS, value)
    ? (value as AccentKey)
    : "default";
}

export interface AccentSwatch {
  key: AccentKey;
  color: string;
}

export function accentSwatches(primaryColor: string | null | undefined): AccentSwatch[] {
  return (Object.keys(ACCENT_PRESETS) as AccentKey[]).map((key) => ({
    key,
    color: ACCENT_PRESETS[key] ?? safePrimaryColor(primaryColor),
  }));
}

export function composeTheme(
  mode: ResolvedThemeMode,
  { accent, primaryColor }: Pick<Appearance, "accent" | "primaryColor">,
): ThemeConfig {
  return {
    algorithm: mode === "dark" ? theme.darkAlgorithm : theme.defaultAlgorithm,
    token: { colorPrimary: ACCENT_PRESETS[accent] ?? safePrimaryColor(primaryColor) },
  };
}
