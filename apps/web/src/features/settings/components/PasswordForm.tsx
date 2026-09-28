import { zodResolver } from "@hookform/resolvers/zod";
import { Alert, Button, Flex, Form, Input, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ApiError, ErrorAlert } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useChangePassword } from "../api/mutations";
import { characters, invalidFields } from "./formErrors";
import { SettingsSection } from "./SettingsSection";

const PASSWORD_FIELDS = ["currentPassword", "newPassword"] as const;

interface PasswordValues {
  currentPassword: string;
  newPassword: string;
  confirmPassword: string;
}

function passwordSchema(t: TFunction<"settings">, minLength: number) {
  const required = t("security.password.validation.required");
  return z
    .object({
      currentPassword: z.string().min(1, required),
      newPassword: z
        .string()
        .min(1, required)
        .refine((value) => characters(value) >= minLength, {
          message: t("security.password.validation.tooShort", { minLength }),
        }),
      confirmPassword: z.string().min(1, required),
    })
    .refine((values) => values.newPassword === values.confirmPassword, {
      path: ["confirmPassword"],
      message: t("security.password.validation.mismatch"),
    });
}

function minLengthOf(error: ApiError): number | null {
  const value = error.details.minLength;
  return typeof value === "number" ? value : null;
}

interface PasswordFormProps {
  passwordMinLength: number | undefined;
}

export function PasswordForm({ passwordMinLength }: PasswordFormProps) {
  const { t } = useTranslation(["settings", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: keyof PasswordValues) => `${idPrefix}-${field}`;
  const [failure, setFailure] = useState<unknown>(null);
  const [changed, setChanged] = useState(false);
  const changePassword = useChangePassword();
  const schema = useMemo(
    () => passwordSchema(t as TFunction<"settings">, passwordMinLength ?? 1),
    [t, passwordMinLength],
  );
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors },
  } = useForm<PasswordValues>({
    resolver: zodResolver(schema),
    defaultValues: { currentPassword: "", newPassword: "", confirmPassword: "" },
  });

  function applyError(error: unknown) {
    if (!(error instanceof ApiError)) {
      setFailure(error);
      return;
    }
    switch (error.code) {
      case "AUTH_RECENT_AUTH_REQUIRED":
        return;
      case "PASSWORD_CURRENT_INVALID":
        setError("currentPassword", {
          type: "server",
          message: t("message.currentPasswordInvalid", { ns: "errors" }),
        });
        setFocus("currentPassword");
        return;
      case "PASSWORD_POLICY_VIOLATION": {
        const minLength = minLengthOf(error);
        setError("newPassword", {
          type: "server",
          message:
            minLength === null
              ? t("message.passwordPolicy", { ns: "errors" })
              : t("security.password.validation.tooShort", { minLength }),
        });
        setFocus("newPassword");
        return;
      }
      default: {
        const fields = invalidFields(error, PASSWORD_FIELDS);
        if (fields.length === 0) {
          setFailure(error);
          return;
        }
        for (const field of fields) {
          setError(field, { type: "server", message: t("security.password.validation.invalid") });
        }
        setFocus(fields[0] ?? "currentPassword");
      }
    }
  }

  function submit({ currentPassword, newPassword }: PasswordValues) {
    if (changePassword.isPending) {
      return;
    }
    setFailure(null);
    setChanged(false);
    changePassword.mutate(
      { currentPassword, newPassword },
      {
        onSuccess: () => {
          reset();
          setChanged(true);
        },
        onError: applyError,
      },
    );
  }

  const pending = changePassword.isPending;

  return (
    <form
      noValidate
      aria-busy={pending}
      onSubmit={(event) => {
        void handleSubmit(submit)(event);
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false} disabled={pending}>
        {failure === null ? null : (
          <div style={{ marginBottom: token.marginLG }}>
            <ErrorAlert error={failure} />
          </div>
        )}
        {changed ? (
          <div style={{ marginBottom: token.marginLG }}>
            <Alert
              type="success"
              showIcon
              role="status"
              title={t("security.password.changed")}
              description={t("security.password.changedDescription")}
            />
          </div>
        ) : null}
        <FormField
          id={id("currentPassword")}
          label={t("security.password.current")}
          error={errors.currentPassword?.message}
        >
          {(fieldProps) => (
            <Controller
              name="currentPassword"
              control={control}
              render={({ field }) => (
                <Input.Password {...field} {...fieldProps} autoComplete="current-password" />
              )}
            />
          )}
        </FormField>
        <FormField
          id={id("newPassword")}
          label={t("security.password.new")}
          error={errors.newPassword?.message}
          {...(passwordMinLength === undefined
            ? {}
            : {
                extra: t("security.password.hint", { minLength: passwordMinLength }),
              })}
        >
          {(fieldProps) => (
            <Controller
              name="newPassword"
              control={control}
              render={({ field }) => (
                <Input.Password {...field} {...fieldProps} autoComplete="new-password" />
              )}
            />
          )}
        </FormField>
        <FormField
          id={id("confirmPassword")}
          label={t("security.password.confirm")}
          error={errors.confirmPassword?.message}
        >
          {(fieldProps) => (
            <Controller
              name="confirmPassword"
              control={control}
              render={({ field }) => (
                <Input.Password {...field} {...fieldProps} autoComplete="new-password" />
              )}
            />
          )}
        </FormField>
        <Flex vertical gap={token.marginSM}>
          <Typography.Text type="secondary">{t("security.password.consequence")}</Typography.Text>
          <div>
            <Button type="primary" htmlType="submit" loading={pending}>
              {t("security.password.submit")}
            </Button>
          </div>
        </Flex>
      </Form>
    </form>
  );
}

interface PasswordSectionProps {
  canChangePassword: boolean;
  hasLocalPassword: boolean;
  passwordMinLength: number | undefined;
}

export function PasswordSection({
  canChangePassword,
  hasLocalPassword,
  passwordMinLength,
}: PasswordSectionProps) {
  const { t } = useTranslation("settings");
  return (
    <SettingsSection
      title={t("security.password.title")}
      description={t("security.password.description")}
      testId="settings-password"
    >
      {canChangePassword ? (
        <div style={{ maxWidth: 480 }}>
          <PasswordForm passwordMinLength={passwordMinLength} />
        </div>
      ) : (
        <Alert
          type="info"
          showIcon
          title={t(
            hasLocalPassword
              ? "security.password.unavailable.disabled"
              : "security.password.unavailable.external",
          )}
        />
      )}
    </SettingsSection>
  );
}
