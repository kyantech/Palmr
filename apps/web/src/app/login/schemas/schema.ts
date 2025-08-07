import { useTranslations } from "next-intl";
import * as z from "zod";

type TFunction = ReturnType<typeof useTranslations>;

export const createLoginSchema = (t: TFunction, passwordAuthEnabled: boolean = true) =>
  z.object({
    emailOrUsername: z.string().min(1, t("validation.emailOrUsernameRequired")),
    password: passwordAuthEnabled ? z.string().min(1, t("validation.passwordRequired")) : z.string().optional(),
  });

export type LoginFormValues = z.infer<ReturnType<typeof createLoginSchema>>;
