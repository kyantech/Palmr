import type { LocaleCode } from "../i18n/catalog";
import { resolveLocale } from "../i18n/resolveLocale";
import { type Appearance, toAccentKey, toThemePreference } from "../theme/appearance";
import type { BootState } from "./bootState";

export interface BootPresentation {
  locale: LocaleCode;
  appearance: Appearance;
}

export function bootPresentation(
  { bootstrap, me }: BootState,
  browserLanguages: readonly string[],
): BootPresentation {
  return {
    locale: resolveLocale({
      authenticatedLocale: me?.user.locale ?? null,
      browserLanguages,
      instanceDefault: bootstrap.defaultLocale,
    }),
    appearance: {
      preference: toThemePreference(me?.user.theme ?? "system"),
      accent: toAccentKey(me?.user.accent ?? "default"),
      primaryColor: bootstrap.primaryColor,
    },
  };
}
