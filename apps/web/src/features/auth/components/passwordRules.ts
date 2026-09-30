import type { TFunction } from "i18next";
import { z } from "zod";
import { ApiError } from "../../../shared/errors";

export function characters(value: string): number {
  return Array.from(value).length;
}

export function newPasswordSchema(t: TFunction<"auth">, minLength: number) {
  const required = t("password.validation.required");
  return z
    .object({
      newPassword: z
        .string()
        .min(1, required)
        .refine((value) => characters(value) >= minLength, {
          message: t("password.validation.tooShort", { minLength }),
        }),
      confirmPassword: z.string().min(1, required),
    })
    .refine((values) => values.newPassword === values.confirmPassword, {
      path: ["confirmPassword"],
      message: t("password.validation.mismatch"),
    });
}

export function policyMinLength(error: ApiError): number | null {
  const value = error.details.minLength;
  return typeof value === "number" ? value : null;
}
