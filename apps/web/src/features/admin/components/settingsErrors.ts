import type { TFunction } from "i18next";
import { ApiError } from "../../../shared/errors";
import { detailNumber, detailText } from "./feedback";

export type SettingsFailure =
  { kind: "field"; key: string; message: string } | { kind: "general"; error: unknown };

export function mapSettingsError(
  error: unknown,
  t: TFunction<"admin">,
  fields: readonly string[],
): SettingsFailure | null {
  if (!(error instanceof ApiError)) {
    return { kind: "general", error };
  }
  if (error.code === "AUTH_RECENT_AUTH_REQUIRED") {
    return null;
  }
  const key = detailText(error, "key");
  if (key === null || !fields.includes(key)) {
    return { kind: "general", error };
  }
  if (error.code === "SETTING_BELOW_FLOOR") {
    const floor = detailNumber(error, "floor");
    return {
      kind: "field",
      key,
      message:
        floor === null
          ? t("settings.errors.belowFloorUnknown")
          : t("settings.errors.belowFloor", { floor }),
    };
  }
  if (error.code === "SETTING_VALUE_INVALID") {
    const max = detailNumber(error, "max");
    if (error.details.requiredWhenEnabled === true) {
      return { kind: "field", key, message: t("settings.errors.requiredWhenEnabled") };
    }
    return {
      kind: "field",
      key,
      message: max === null ? t("settings.errors.invalid") : t("settings.errors.aboveMax", { max }),
    };
  }
  return { kind: "general", error };
}

export function changedKeys<Settings extends object>(
  current: Settings,
  candidate: { [Key in keyof Settings]?: Settings[Key] },
): { [Key in keyof Settings]?: Settings[Key] } {
  const patch: { [Key in keyof Settings]?: Settings[Key] } = {};
  for (const key of Object.keys(candidate) as (keyof Settings)[]) {
    if (candidate[key] !== current[key]) {
      patch[key] = candidate[key];
    }
  }
  return patch;
}
