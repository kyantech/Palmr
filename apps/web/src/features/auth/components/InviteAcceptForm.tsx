import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, Select, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, type FieldPath, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ApiError, ErrorAlert, presentError } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { localeOptions } from "../../../shared/format/locale";
import { FormField } from "../../../shared/ui/FormField";
import { type AcceptInviteRequest, useAcceptInvite } from "../api/mutations";
import type { InviteLookup } from "../api/queries";
import { characters, policyMinLength } from "./passwordRules";

const DISPLAY_TEXT_MAX = 100;
const USERNAME_MIN = 3;
const USERNAME_MAX = 64;

interface InviteValues extends AcceptInviteRequest {
  confirmPassword: string;
}

type InviteField = FieldPath<InviteValues>;

const NAME_FIELDS = ["firstName", "lastName"] as const;

const PASSWORD_FIELDS = ["password", "confirmPassword"] as const;

const SERVER_FIELDS: readonly InviteField[] = [
  "firstName",
  "lastName",
  "username",
  "password",
  "locale",
];

function inviteSchema(t: TFunction<"auth">, minLength: number) {
  const required = t("invite.validation.required");
  const displayText = z
    .string()
    .trim()
    .min(1, required)
    .refine((value) => characters(value) <= DISPLAY_TEXT_MAX, {
      message: t("invite.validation.tooLong", { max: DISPLAY_TEXT_MAX }),
    });
  return z
    .object({
      firstName: displayText,
      lastName: displayText,
      username: z
        .string()
        .min(1, required)
        .refine((value) => characters(value) >= USERNAME_MIN && characters(value) <= USERNAME_MAX, {
          message: t("invite.validation.usernameLength", { min: USERNAME_MIN, max: USERNAME_MAX }),
        }),
      password: z
        .string()
        .min(1, required)
        .refine((value) => characters(value) >= minLength, {
          message: t("password.validation.tooShort", { minLength }),
        }),
      confirmPassword: z.string().min(1, required),
      locale: z.string().min(1, required),
    })
    .refine((values) => values.password === values.confirmPassword, {
      path: ["confirmPassword"],
      message: t("password.validation.mismatch"),
    });
}

function isServerField(value: string): value is InviteField {
  return (SERVER_FIELDS as readonly string[]).includes(value);
}

export interface InviteAcceptFormProps {
  token: string;
  invite: InviteLookup;
  supportedLocales: readonly string[];
  defaultLocale: string;
  onAccepted: () => Promise<void>;
  onTerminal: (error: unknown) => boolean;
}

export function InviteAcceptForm({
  token,
  invite,
  supportedLocales,
  defaultLocale,
  onAccepted,
  onTerminal,
}: InviteAcceptFormProps) {
  const { t, i18n } = useTranslation(["auth", "errors"]);
  const { token: design } = theme.useToken();
  const idPrefix = useId();
  const id = (field: InviteField | "email") => `${idPrefix}-${field}`;
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const accept = useAcceptInvite(token);
  const options = useMemo(() => localeOptions(supportedLocales), [supportedLocales]);
  const schema = useMemo(
    () => inviteSchema(t as TFunction<"auth">, invite.passwordMinLength),
    [t, invite.passwordMinLength],
  );
  const {
    control,
    handleSubmit,
    setError,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<InviteValues>({
    resolver: zodResolver(schema),
    defaultValues: {
      firstName: "",
      lastName: "",
      username: "",
      password: "",
      confirmPassword: "",
      locale: supportedLocales.includes(defaultLocale)
        ? defaultLocale
        : (supportedLocales[0] ?? ""),
    },
  });

  useEffect(() => {
    setFocus("firstName");
  }, [setFocus]);

  function applyError(error: unknown) {
    if (onTerminal(error)) {
      return;
    }
    if (!(error instanceof ApiError)) {
      setFailure(error);
      return;
    }
    switch (error.code) {
      case "USER_USERNAME_TAKEN":
        setError("username", {
          type: "server",
          message: t("message.usernameTaken", { ns: "errors" }),
        });
        setFocus("username");
        return;
      case "PASSWORD_POLICY_VIOLATION": {
        const minLength = policyMinLength(error);
        setError("password", {
          type: "server",
          message:
            minLength === null
              ? t("message.passwordPolicy", { ns: "errors" })
              : t("password.validation.tooShort", { minLength }),
        });
        setFocus("password");
        return;
      }
      case "VALIDATION_ERROR": {
        const fields = Array.isArray(error.details.fields) ? error.details.fields : [];
        const invalid = fields.filter(isServerField);
        if (invalid.length === 0) {
          setFailure(error);
          return;
        }
        for (const field of invalid) {
          setError(field, { type: "server", message: t("invite.validation.invalid") });
        }
        setFocus(invalid[0] ?? "firstName");
        return;
      }
      default:
        if (!presentError(error).presentation.silent) {
          setFailure(error);
        }
    }
  }

  async function submit(values: InviteValues) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      try {
        await accept.mutateAsync({
          firstName: values.firstName.trim(),
          lastName: values.lastName.trim(),
          username: values.username,
          password: values.password,
          locale: values.locale,
        });
      } catch (error) {
        applyError(error);
        return;
      }
      await onAccepted();
    } catch (error) {
      setFailure(error);
    } finally {
      inFlight.current = false;
    }
  }

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
          <div style={{ marginBottom: design.marginLG }}>
            <ErrorAlert error={failure} />
          </div>
        )}
        <Flex
          vertical
          gap={2}
          style={{
            marginBottom: design.marginLG,
            padding: `${String(design.paddingSM)}px ${String(design.padding)}px`,
            background: design.colorFillQuaternary,
            border: `${String(design.lineWidth)}px ${design.lineType} ${design.colorBorderSecondary}`,
            borderRadius: design.borderRadiusLG,
          }}
        >
          <Typography.Text
            id={id("email")}
            type="secondary"
            style={{ fontSize: design.fontSizeSM }}
          >
            {t("invite.email")}
          </Typography.Text>
          <Typography.Text
            strong
            aria-labelledby={id("email")}
            data-testid="invite-email"
            style={{ wordBreak: "break-all" }}
          >
            {invite.email}
          </Typography.Text>
          <Typography.Text type="secondary" style={{ fontSize: design.fontSizeSM }}>
            {t("invite.emailBound", {
              date: formatDateTime(invite.expiresAt, i18n.language),
            })}
          </Typography.Text>
        </Flex>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fit, minmax(10rem, 1fr))",
            columnGap: design.marginSM,
          }}
        >
          {NAME_FIELDS.map((name) => (
            <FormField
              key={name}
              id={id(name)}
              label={t(`invite.${name}`)}
              error={errors[name]?.message}
            >
              {(aria) => (
                <Controller
                  name={name}
                  control={control}
                  render={({ field }) => (
                    <Input
                      {...field}
                      {...aria}
                      size="large"
                      autoComplete={name === "firstName" ? "given-name" : "family-name"}
                    />
                  )}
                />
              )}
            </FormField>
          ))}
        </div>
        <FormField
          id={id("username")}
          label={t("invite.username")}
          error={errors.username?.message}
        >
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
        {PASSWORD_FIELDS.map((name) => (
          <FormField
            key={name}
            id={id(name)}
            label={t(name === "password" ? "invite.password" : "password.confirm")}
            error={errors[name]?.message}
            {...(name === "password"
              ? { extra: t("password.hint", { minLength: invite.passwordMinLength }) }
              : {})}
          >
            {(aria) => (
              <Controller
                name={name}
                control={control}
                render={({ field }) => (
                  <Input.Password {...field} {...aria} size="large" autoComplete="new-password" />
                )}
              />
            )}
          </FormField>
        ))}
        <FormField id={id("locale")} label={t("invite.locale")} error={errors.locale?.message}>
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
        <Button
          type="primary"
          htmlType="submit"
          size="large"
          block
          loading={isSubmitting}
          style={{ marginTop: design.marginXS }}
        >
          {t("invite.submit")}
        </Button>
      </Form>
    </form>
  );
}
