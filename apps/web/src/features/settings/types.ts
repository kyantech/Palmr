import type { components } from "../../shared/api/schema";

export type Me = components["schemas"]["MeResponse"];
export type Profile = components["schemas"]["MeUser"];
export type Preferences = components["schemas"]["Preferences"];
export type SessionItem = components["schemas"]["SessionItem"];
export type SessionPage = components["schemas"]["Page_SessionItem"];
export type EffectiveSettings = components["schemas"]["EffectiveSettings"];

export const THEME_OPTIONS = ["light", "dark", "system"] as const;

export type ThemeOption = (typeof THEME_OPTIONS)[number];

export interface AccentPreset {
  key: string;
  color: string;
}

export interface ProfileChange {
  firstName: string;
  lastName: string;
}

export interface PasswordChange {
  currentPassword: string;
  newPassword: string;
}

export type PreferenceChange = { theme: ThemeOption } | { accent: string } | { locale: string };
