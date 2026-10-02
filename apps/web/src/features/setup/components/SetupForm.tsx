import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Divider, Flex, Form, Input, Select, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, type FieldPath, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import {
  ApiError,
  detailFields,
  ErrorAlert,
  type ErrorCode,
  isApiErrorCode,
  presentError,
} from "../../../shared/errors";
import { localeOptions } from "../../../shared/format/locale";
import { FormField } from "../../../shared/ui/FormField";
import { type SetupRequest, useCompleteSetup } from "../api/mutations";

export const DEFAULT_APP_NAME = "Palmr";

const DISPLAY_TEXT_MAX = 100;
const USERNAME_MIN = 3;
const USERNAME_MAX = 64;
const EMAIL_MAX = 254;
const EMAIL_SHAPE = /^[^\s@]+@[^\s@]+$/;

type SetupField = FieldPath<SetupRequest>;

const SETUP_FIELDS: readonly SetupField[] = [
  "appName",
  "locale",
  "firstName",
  "lastName",
  "username",
  "email",
  "password",
];

const FIELD_ERRORS: Partial<Record<ErrorCode, SetupField>> = {
  USER_EMAIL_TAKEN: "email",
  USER_USERNAME_TAKEN: "username",
  PASSWORD_POLICY_VIOLATION: "password",
};

function characters(value: string): number {
  return Array.from(value).length;
}

function setupSchema(t: TFunction<"setup">, passwordMinLength: number) {
  const required = t("validation.required");
  const displayText = z
    .string()
    .trim()
    .min(1, required)
    .refine((value) => characters(value) <= DISPLAY_TEXT_MAX, {
      message: t("validation.tooLong", { max: DISPLAY_TEXT_MAX }),
    });
  return z.object({
    appName: displayText,
    firstName: displayText,
    lastName: displayText,
    username: z
      .string()
      .min(1, required)
      .refine((value) => characters(value) >= USERNAME_MIN && characters(value) <= USERNAME_MAX, {
        message: t("validation.usernameLength", { min: USERNAME_MIN, max: USERNAME_MAX }),
      }),
    email: z
      .string()
      .min(1, required)
      .refine((value) => characters(value) <= EMAIL_MAX && EMAIL_SHAPE.test(value), {
        message: t("validation.email"),
      }),
    password: z
      .string()
      .min(1, required)
      .refine((value) => characters(value) >= passwordMinLength, {
        message: t("validation.passwordLength", { minLength: passwordMinLength }),
      }),
    locale: z.string().min(1, required),
  });
}

function isSetupField(value: string): value is SetupField {
  return (SETUP_FIELDS as readonly string[]).includes(value);
}

interface SetupFormProps {
  defaultLocale: string;
  supportedLocales: readonly string[];
  passwordMinLength: number | undefined;
  onSetupFinished: () => Promise<void>;
}

export function SetupForm({
  defaultLocale,
  supportedLocales,
  passwordMinLength,
  onSetupFinished,
}: SetupFormProps) {
  const { t } = useTranslation(["setup", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: SetupField) => `${idPrefix}-${field}`;
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const completeSetup = useCompleteSetup();
  const policyReady = passwordMinLength !== undefined;
  const schema = useMemo(
    () => setupSchema(t as TFunction<"setup">, passwordMinLength ?? 0),
    [t, passwordMinLength],
  );
  const options = useMemo(() => localeOptions(supportedLocales), [supportedLocales]);
  const {
    control,
    handleSubmit,
    setError,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<SetupRequest>({
    resolver: zodResolver(schema),
    defaultValues: {
      appName: DEFAULT_APP_NAME,
      firstName: "",
      lastName: "",
      username: "",
      email: "",
      password: "",
      locale: defaultLocale,
    },
  });

  useEffect(() => {
    setFocus("appName");
  }, [setFocus]);

  function applyFieldErrors(error: unknown): boolean {
    if (!(error instanceof ApiError)) {
      return false;
    }
    const field = FIELD_ERRORS[error.code];
    if (field !== undefined) {
      setError(field, {
        type: "server",
        message: t(presentError(error).presentation.i18nKey, { ns: "errors" }),
      });
      setFocus(field);
      return true;
    }
    if (error.code === "VALIDATION_ERROR") {
      const invalid = detailFields(error).filter(isSetupField);
      invalid.forEach((name) => {
        setError(name, { type: "server", message: t("validation.invalid") });
      });
      const [first] = invalid;
      if (first !== undefined) {
        setFocus(first);
      }
    }
    return false;
  }

  async function submit(values: SetupRequest) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      try {
        await completeSetup.mutateAsync(values);
      } catch (error) {
        if (isApiErrorCode(error, "SETUP_ALREADY_COMPLETED")) {
          setFailure(error);
          await onSetupFinished();
          return;
        }
        if (!applyFieldErrors(error)) {
          setFailure(error);
        }
        return;
      }
      await onSetupFinished();
    } catch (error) {
      setFailure(error);
    } finally {
      inFlight.current = false;
    }
  }

  const sectionTitle = {
    level: 2 as const,
    style: {
      margin: 0,
      marginBottom: token.marginSM,
      fontSize: token.fontSize,
      lineHeight: token.lineHeight,
      fontWeight: token.fontWeightStrong,
    },
  };

  return (
    <form
      noValidate
      aria-busy={isSubmitting}
      onSubmit={(event) => {
        void handleSubmit(submit)(event);
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false} disabled={isSubmitting}>
        {failure === null ? null : (
          <div style={{ marginBottom: token.marginLG }}>
            <ErrorAlert error={failure} />
          </div>
        )}

        <section aria-labelledby={`${idPrefix}-instance`}>
          <Typography.Title id={`${idPrefix}-instance`} {...sectionTitle}>
            {t("instanceSection")}
          </Typography.Title>
          <FormField id={id("appName")} label={t("appName")} error={errors.appName?.message}>
            {(aria) => (
              <Controller
                name="appName"
                control={control}
                render={({ field }) => (
                  <Input {...field} {...aria} size="large" autoComplete="organization" />
                )}
              />
            )}
          </FormField>
          <FormField
            id={id("locale")}
            label={t("locale")}
            error={errors.locale?.message}
            extra={t("localeHelp")}
          >
            {(aria) => (
              <Controller
                name="locale"
                control={control}
                render={({ field: { ref, value, onChange, onBlur } }) => (
                  <Select
                    ref={ref}
                    {...aria}
                    size="large"
                    value={value}
                    onChange={onChange}
                    onBlur={onBlur}
                    options={options}
                    showSearch={{ optionFilterProp: "label" }}
                  />
                )}
              />
            )}
          </FormField>
        </section>

        <Divider style={{ marginBlock: token.marginXS, marginBlockEnd: token.marginLG }} />

        <section aria-labelledby={`${idPrefix}-account`}>
          <Typography.Title id={`${idPrefix}-account`} {...sectionTitle}>
            {t("accountSection")}
          </Typography.Title>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(10rem, 1fr))",
              columnGap: token.marginSM,
            }}
          >
            <FormField
              id={id("firstName")}
              label={t("firstName")}
              error={errors.firstName?.message}
            >
              {(aria) => (
                <Controller
                  name="firstName"
                  control={control}
                  render={({ field }) => (
                    <Input {...field} {...aria} size="large" autoComplete="given-name" />
                  )}
                />
              )}
            </FormField>

            <FormField id={id("lastName")} label={t("lastName")} error={errors.lastName?.message}>
              {(aria) => (
                <Controller
                  name="lastName"
                  control={control}
                  render={({ field }) => (
                    <Input {...field} {...aria} size="large" autoComplete="family-name" />
                  )}
                />
              )}
            </FormField>
          </div>
          <FormField id={id("username")} label={t("username")} error={errors.username?.message}>
            {(aria) => (
              <Controller
                name="username"
                control={control}
                render={({ field }) => (
                  <Input
                    {...field}
                    {...aria}
                    size="large"
                    autoComplete="username"
                    autoCapitalize="none"
                    spellCheck={false}
                  />
                )}
              />
            )}
          </FormField>
          <FormField id={id("email")} label={t("email")} error={errors.email?.message}>
            {(aria) => (
              <Controller
                name="email"
                control={control}
                render={({ field }) => (
                  <Input
                    {...field}
                    {...aria}
                    size="large"
                    type="email"
                    inputMode="email"
                    autoComplete="email"
                    autoCapitalize="none"
                    spellCheck={false}
                  />
                )}
              />
            )}
          </FormField>
          <FormField
            id={id("password")}
            label={t("password")}
            error={errors.password?.message}
            extra={
              <span style={{ display: "inline-block", minHeight: "1lh" }}>
                {policyReady ? t("passwordHint", { minLength: passwordMinLength }) : null}
              </span>
            }
          >
            {(aria) => (
              <Controller
                name="password"
                control={control}
                render={({ field }) => (
                  <Input.Password {...field} {...aria} size="large" autoComplete="new-password" />
                )}
              />
            )}
          </FormField>
        </section>

        <Flex vertical style={{ marginTop: token.marginXS }}>
          <Button
            type="primary"
            htmlType="submit"
            size="large"
            block
            loading={isSubmitting}
            disabled={!policyReady}
          >
            {t("submit")}
          </Button>
        </Flex>
      </Form>
    </form>
  );
}
