import type { TFunction } from "i18next";
import { z } from "zod";

export function requiredInteger(t: TFunction<"admin">) {
  return z
    .number()
    .nullable()
    .superRefine((value, context) => {
      if (value === null) {
        context.addIssue({ code: "custom", message: t("validation.required") });
      } else if (!Number.isInteger(value)) {
        context.addIssue({ code: "custom", message: t("validation.integer") });
      }
    });
}
