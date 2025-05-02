import { IconUserEdit } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ProfileFormProps } from "../types";

export function ProfileForm({ form, onSubmit }: ProfileFormProps) {
  const t = useTranslations();
  const {
    register,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = form;

  return (
    <Card>
      <CardHeader>
        <h2 className="text-lg font-semibold">{t("profile.form.title")}</h2>
      </CardHeader>
      <CardContent>
        <form className="flex flex-col gap-4" onSubmit={handleSubmit(onSubmit)}>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div className="flex flex-col gap-2">
              <Label>{t("profile.form.firstName")}</Label>
              <Input
                {...register("firstName")}
                className={errors.firstName ? "border-red-500" : ""}
                aria-invalid={!!errors.firstName}
                aria-errormessage={errors.firstName?.message}
                placeholder={t("profile.form.firstName")}
              />
              {errors.firstName && <span className="text-sm text-red-500">{errors.firstName.message}</span>}
            </div>
            <div className="flex flex-col gap-2">
              <Label>{t("profile.form.lastName")}</Label>
              <Input
                {...register("lastName")}
                className={errors.lastName ? "border-red-500" : ""}
                aria-invalid={!!errors.lastName}
                aria-errormessage={errors.lastName?.message}
                placeholder={t("profile.form.lastName")}
              />
              {errors.lastName && <span className="text-sm text-red-500">{errors.lastName.message}</span>}
            </div>
          </div>
          <div className="flex flex-col gap-2">
            <Label>{t("profile.form.username")}</Label>
            <Input
              {...register("username")}
              className={errors.username ? "border-red-500" : ""}
              aria-invalid={!!errors.username}
              aria-errormessage={errors.username?.message}
              placeholder={t("profile.form.username")}
            />
            {errors.username && <span className="text-sm text-red-500">{errors.username.message}</span>}
          </div>
          <div className="flex flex-col gap-2">
            <Label>{t("profile.form.email")}</Label>
            <Input
              {...register("email")}
              type="email"
              className={errors.email ? "border-red-500" : ""}
              aria-invalid={!!errors.email}
              aria-errormessage={errors.email?.message}
              placeholder={t("profile.form.email")}
            />
            {errors.email && <span className="text-sm text-red-500">{errors.email.message}</span>}
          </div>
          <div className="flex justify-end">
            <Button className="mt-4 font-semibold flex items-center gap-2" disabled={isSubmitting} type="submit">
              {!isSubmitting && <IconUserEdit className="w-5 h-5" />}
              {t("profile.form.updateButton")}
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
