import { useEffect, useState } from "react";
import Link from "next/link";
import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";

import { Button } from "@/components/ui/button";
import { Form, FormControl, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { getEnabledProviders } from "@/http/endpoints";
import { createLoginSchema, type LoginFormValues } from "../schemas/schema";
import { MultiProviderButtons } from "./multi-provider-buttons";
import { PasswordVisibilityToggle } from "./password-visibility-toggle";

interface LoginFormProps {
  error?: string;
  isVisible: boolean;
  onToggleVisibility: () => void;
  onSubmit: (data: LoginFormValues) => Promise<void>;
  passwordAuthEnabled: boolean;
  authConfigLoading: boolean;
}

export function LoginForm({
  error,
  isVisible,
  onToggleVisibility,
  onSubmit,
  passwordAuthEnabled,
  authConfigLoading,
}: LoginFormProps) {
  const t = useTranslations();
  const [hasEnabledProviders, setHasEnabledProviders] = useState(false);
  const [providersLoading, setProvidersLoading] = useState(true);

  const loginSchema = createLoginSchema(t, passwordAuthEnabled);
  const form = useForm<LoginFormValues>({
    resolver: zodResolver(loginSchema),
    defaultValues: {
      emailOrUsername: "",
      password: passwordAuthEnabled ? "" : undefined,
    },
  });

  const isSubmitting = form.formState.isSubmitting;

  useEffect(() => {
    const checkProviders = async () => {
      try {
        const response = await getEnabledProviders();
        const data = response.data as any;
        setHasEnabledProviders(data.success && data.data && data.data.length > 0);
      } catch (error) {
        console.error("Error checking providers:", error);
        setHasEnabledProviders(false);
      } finally {
        setProvidersLoading(false);
      }
    };

    checkProviders();
  }, []);

  const renderErrorMessage = () =>
    error && (
      <p className="text-destructive text-sm text-center bg-destructive/10 p-2 rounded-md">
        {error.replace("errors.", "")}
      </p>
    );

  const renderEmailOrUsernameField = () => (
    <FormField
      control={form.control}
      name="emailOrUsername"
      render={({ field }) => (
        <FormItem>
          <FormLabel>{t("login.emailOrUsernameLabel")}</FormLabel>
          <FormControl className="-mb-1">
            <Input
              {...field}
              type="text"
              placeholder={t("login.emailOrUsernamePlaceholder")}
              disabled={isSubmitting}
              className="bg-transparent backdrop-blur-md"
            />
          </FormControl>
          <FormMessage />
        </FormItem>
      )}
    />
  );

  const renderPasswordField = () => (
    <FormField
      control={form.control}
      name="password"
      render={({ field }) => (
        <FormItem>
          <FormLabel>{t("login.passwordLabel")}</FormLabel>
          <FormControl>
            <div className="relative">
              <Input
                {...field}
                type={isVisible ? "text" : "password"}
                placeholder={t("login.passwordPlaceholder")}
                disabled={isSubmitting}
                className="bg-transparent backdrop-blur-md pr-10"
              />
              <PasswordVisibilityToggle isVisible={isVisible} onToggle={onToggleVisibility} />
            </div>
          </FormControl>
          <FormMessage />
        </FormItem>
      )}
    />
  );

  if (authConfigLoading || providersLoading) {
    return (
      <div className="flex justify-center items-center py-8">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
      </div>
    );
  }

  if (!passwordAuthEnabled && hasEnabledProviders) {
    return (
      <>
        {renderErrorMessage()}
        <MultiProviderButtons showSeparator={false} />
      </>
    );
  }

  if (!passwordAuthEnabled && !hasEnabledProviders) {
    return (
      <>
        {renderErrorMessage()}
        <div className="text-center py-8">
          <p className="text-destructive text-sm">{t("login.noAuthMethodsAvailable")}</p>
        </div>
      </>
    );
  }

  return (
    <>
      {renderErrorMessage()}
      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
          {renderEmailOrUsernameField()}
          {passwordAuthEnabled && renderPasswordField()}
          <Button className="w-full mt-4 cursor-pointer" variant="default" size="lg" type="submit">
            {isSubmitting ? t("login.signingIn") : t("login.signIn")}
          </Button>
        </form>
      </Form>

      <MultiProviderButtons />

      {passwordAuthEnabled && (
        <div className="flex w-full items-center justify-center px-1 mt-2">
          <Link className="text-muted-foreground hover:text-primary text-sm" href="/forgot-password">
            {t("login.forgotPassword")}
          </Link>
        </div>
      )}
    </>
  );
}
