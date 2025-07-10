"use client";

import { useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { z } from "zod";

import { Button } from "@/components/ui/button";
import { Form, FormControl, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { useAppInfo } from "@/contexts/app-info-context";
import { registerUser, updateConfig } from "@/http/endpoints";
import { MultiProviderButtons } from "./multi-provider-buttons";
import { PasswordVisibilityToggle } from "./password-visibility-toggle";

interface RegisterFormProps {
  isVisible: boolean;
  onToggleVisibility: () => void;
}

export function RegisterForm({ isVisible, onToggleVisibility }: RegisterFormProps) {
  const t = useTranslations();
  const { refreshAppInfo } = useAppInfo();
  const [error, setError] = useState<string | null>(null);

  const registerSchema = z.object({
    firstName: z.string().min(1, t("register.validation.firstNameRequired")),
    lastName: z.string().min(1, t("register.validation.lastNameRequired")),
    username: z.string().min(3, t("register.validation.usernameMinLength")),
    email: z.string().email(t("register.validation.invalidEmail")),
    password: z.string().min(8, t("register.validation.passwordMinLength")),
  });

  const form = useForm<z.infer<typeof registerSchema>>({
    resolver: zodResolver(registerSchema),
    defaultValues: {
      firstName: "",
      lastName: "",
      username: "",
      email: "",
      password: "",
    },
  });

  const onSubmit = async (data: z.infer<typeof registerSchema>) => {
    setError(null);
    try {
      await registerUser({
        ...data,
      });

      await updateConfig("firstUserAccess", {
        value: "false",
      });

      await refreshAppInfo();
      toast.success(t("register.validation.success"));
    } catch {
      setError(t("register.validation.error"));
      toast.error(t("register.validation.error"));
    }
  };

  const renderErrorMessage = () => (
    <p className="text-destructive text-sm text-center bg-destructive/10 p-2 rounded-md">{error}</p>
  );

  const renderForm = () => (
    <Form {...form}>
      <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
        <FormField
          control={form.control}
          name="firstName"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.firstName")}</FormLabel>
              <FormControl className="-mb-1">
                <Input
                  {...field}
                  placeholder={t("register.labels.firstName")}
                  disabled={form.formState.isSubmitting}
                  className="bg-transparent backdrop-blur-md"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="lastName"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.lastName")}</FormLabel>
              <FormControl className="-mb-1">
                <Input
                  {...field}
                  placeholder={t("register.labels.lastName")}
                  disabled={form.formState.isSubmitting}
                  className="bg-transparent backdrop-blur-md"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="username"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.username")}</FormLabel>
              <FormControl className="-mb-1">
                <Input
                  {...field}
                  placeholder={t("register.labels.username")}
                  disabled={form.formState.isSubmitting}
                  className="bg-transparent backdrop-blur-md"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="email"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.email")}</FormLabel>
              <FormControl className="-mb-1">
                <Input
                  {...field}
                  type="email"
                  placeholder={t("register.labels.email")}
                  disabled={form.formState.isSubmitting}
                  className="bg-transparent backdrop-blur-md"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={form.control}
          name="password"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t("register.labels.password")}</FormLabel>
              <FormControl>
                <div className="relative">
                  <Input
                    {...field}
                    type={isVisible ? "text" : "password"}
                    placeholder={t("register.labels.password")}
                    disabled={form.formState.isSubmitting}
                    className="bg-transparent backdrop-blur-md pr-10"
                  />
                  <PasswordVisibilityToggle isVisible={isVisible} onToggle={onToggleVisibility} />
                </div>
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <Button className="w-full mt-4 cursor-pointer" variant="default" size="lg" type="submit">
          {form.formState.isSubmitting ? t("register.buttons.creating") : t("register.buttons.createAdmin")}
        </Button>
      </form>
    </Form>
  );

  return (
    <>
      {error && renderErrorMessage()}
      {renderForm()}
      <MultiProviderButtons />
    </>
  );
}
