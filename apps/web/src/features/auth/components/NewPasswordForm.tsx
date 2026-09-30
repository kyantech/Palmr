import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { type ReactNode, useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { ApiError, ErrorAlert, presentError } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { newPasswordSchema, policyMinLength } from "./passwordRules";

interface NewPasswordValues {
  newPassword: string;
  confirmPassword: string;
}

const FIELDS = ["newPassword", "confirmPassword"] as const;

export interface NewPasswordFormProps {
  minLength: number | undefined;
  submitLabel: string;
  consequence?: string;
  secondaryAction?: ReactNode;
  submit: (newPassword: string) => Promise<unknown>;
  interceptError?: (error: unknown) => boolean;
}

export function NewPasswordForm({
  minLength,
  submitLabel,
  consequence,
  secondaryAction,
  submit,
  interceptError,
}: NewPasswordFormProps) {
  const { t } = useTranslation(["auth", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const schema = useMemo(
    () => newPasswordSchema(t as TFunction<"auth">, minLength ?? 1),
    [t, minLength],
  );
  const {
    control,
    handleSubmit,
    setError,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<NewPasswordValues>({
    resolver: zodResolver(schema),
    defaultValues: { newPassword: "", confirmPassword: "" },
  });

  useEffect(() => {
    setFocus("newPassword");
  }, [setFocus]);

  function applyError(error: unknown) {
    if (interceptError?.(error) === true) {
      return;
    }
    if (error instanceof ApiError && error.code === "PASSWORD_POLICY_VIOLATION") {
      const required = policyMinLength(error);
      setError("newPassword", {
        type: "server",
        message:
          required === null
            ? t("message.passwordPolicy", { ns: "errors" })
            : t("password.validation.tooShort", { minLength: required }),
      });
      setFocus("newPassword");
      return;
    }
    if (error instanceof ApiError && error.code === "VALIDATION_ERROR") {
      const fields = Array.isArray(error.details.fields) ? error.details.fields : [];
      if (fields.includes("newPassword")) {
        setError("newPassword", { type: "server", message: t("password.validation.invalid") });
        setFocus("newPassword");
        return;
      }
    }
    if (!presentError(error).presentation.silent) {
      setFailure(error);
    }
  }

  async function onSubmit({ newPassword }: NewPasswordValues) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      await submit(newPassword);
    } catch (error) {
      applyError(error);
    } finally {
      inFlight.current = false;
    }
  }

  return (
    <form
      noValidate
      aria-busy={isSubmitting}
      onSubmit={(event) => {
        void handleSubmit(onSubmit)(event);
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false}>
        {failure === null ? null : (
          <div style={{ marginBottom: token.marginLG }}>
            <ErrorAlert error={failure} />
          </div>
        )}
        {FIELDS.map((name) => (
          <FormField
            key={name}
            id={`${idPrefix}-${name}`}
            label={t(name === "newPassword" ? "password.new" : "password.confirm")}
            error={errors[name]?.message}
            {...(name === "newPassword" && minLength !== undefined
              ? { extra: t("password.hint", { minLength }) }
              : {})}
          >
            {(aria) => (
              <Controller
                name={name}
                control={control}
                render={({ field }) => (
                  <Input.Password
                    {...field}
                    {...aria}
                    size="large"
                    autoComplete="new-password"
                    readOnly={isSubmitting}
                  />
                )}
              />
            )}
          </FormField>
        ))}
        <Flex vertical gap={token.marginSM} style={{ marginTop: token.marginXS }}>
          {consequence === undefined ? null : (
            <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
              {consequence}
            </Typography.Text>
          )}
          <Button type="primary" htmlType="submit" size="large" block loading={isSubmitting}>
            {submitLabel}
          </Button>
          {secondaryAction}
        </Flex>
      </Form>
    </form>
  );
}
