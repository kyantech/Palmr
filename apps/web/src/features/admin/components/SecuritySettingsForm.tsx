import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Divider, Flex, Form, InputNumber, Switch, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { FormField } from "../../../shared/ui/FormField";
import { useUpdateSecuritySettings } from "../api/mutations";
import type { SecurityPatch, SecuritySettings } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { requiredInteger } from "./fieldSchemas";
import { changedKeys, mapSettingsError } from "./settingsErrors";

type NumericKey =
  | "passwordMinLength"
  | "publicLinkPasswordMinLength"
  | "maxLoginAttempts"
  | "loginLockoutMinutes"
  | "sessionIdleDays"
  | "sessionAbsoluteDays"
  | "recentAuthMinutes"
  | "passwordResetValidityMinutes"
  | "inviteValidityHours"
  | "trustedDeviceDurationDays";

type FlagKey = "twoFactorRequired" | "trustedDevicesEnabled";

type Unit = "characters" | "attempts" | "minutes" | "hours" | "days";

interface NumericField {
  key: NumericKey;
  unit: Unit;
}

interface Group {
  key: string;
  fields: readonly NumericField[];
}

const GROUPS: readonly Group[] = [
  {
    key: "passwords",
    fields: [
      { key: "passwordMinLength", unit: "characters" },
      { key: "publicLinkPasswordMinLength", unit: "characters" },
    ],
  },
  {
    key: "signIn",
    fields: [
      { key: "maxLoginAttempts", unit: "attempts" },
      { key: "loginLockoutMinutes", unit: "minutes" },
    ],
  },
  {
    key: "sessions",
    fields: [
      { key: "sessionIdleDays", unit: "days" },
      { key: "sessionAbsoluteDays", unit: "days" },
      { key: "recentAuthMinutes", unit: "minutes" },
    ],
  },
  {
    key: "links",
    fields: [
      { key: "passwordResetValidityMinutes", unit: "minutes" },
      { key: "inviteValidityHours", unit: "hours" },
    ],
  },
];

const TRUSTED_DEVICE_DURATION: NumericField = { key: "trustedDeviceDurationDays", unit: "days" };

const FIELD_KEYS: readonly string[] = [
  ...GROUPS.flatMap((group) => group.fields.map((field) => field.key)),
  TRUSTED_DEVICE_DURATION.key,
  "twoFactorRequired",
  "trustedDevicesEnabled",
];

type SecurityValues = Record<NumericKey, number | null> & Record<FlagKey, boolean>;

function valuesOf(settings: SecuritySettings): SecurityValues {
  return {
    passwordMinLength: settings.passwordMinLength,
    publicLinkPasswordMinLength: settings.publicLinkPasswordMinLength,
    maxLoginAttempts: settings.maxLoginAttempts,
    loginLockoutMinutes: settings.loginLockoutMinutes,
    sessionIdleDays: settings.sessionIdleDays,
    sessionAbsoluteDays: settings.sessionAbsoluteDays,
    recentAuthMinutes: settings.recentAuthMinutes,
    passwordResetValidityMinutes: settings.passwordResetValidityMinutes,
    inviteValidityHours: settings.inviteValidityHours,
    trustedDeviceDurationDays: settings.trustedDeviceDurationDays,
    twoFactorRequired: settings.twoFactorRequired,
    trustedDevicesEnabled: settings.trustedDevicesEnabled,
  };
}

const NUMERIC_KEYS: readonly NumericKey[] = [
  "passwordMinLength",
  "publicLinkPasswordMinLength",
  "maxLoginAttempts",
  "loginLockoutMinutes",
  "sessionIdleDays",
  "sessionAbsoluteDays",
  "recentAuthMinutes",
  "passwordResetValidityMinutes",
  "inviteValidityHours",
  "trustedDeviceDurationDays",
];

function candidateOf(values: SecurityValues): Partial<SecuritySettings> {
  const candidate: Partial<SecuritySettings> = {
    twoFactorRequired: values.twoFactorRequired,
    trustedDevicesEnabled: values.trustedDevicesEnabled,
  };
  for (const key of NUMERIC_KEYS) {
    const value = values[key];
    if (value !== null) {
      candidate[key] = value;
    }
  }
  return candidate;
}

function securitySchema(t: TFunction<"admin">) {
  const number = () => requiredInteger(t);
  return z.object({
    passwordMinLength: number(),
    publicLinkPasswordMinLength: number(),
    maxLoginAttempts: number(),
    loginLockoutMinutes: number(),
    sessionIdleDays: number(),
    sessionAbsoluteDays: number(),
    recentAuthMinutes: number(),
    passwordResetValidityMinutes: number(),
    inviteValidityHours: number(),
    trustedDeviceDurationDays: number(),
    twoFactorRequired: z.boolean(),
    trustedDevicesEnabled: z.boolean(),
  });
}

interface SecuritySettingsFormProps {
  settings: SecuritySettings;
}

export function SecuritySettingsForm({ settings }: SecuritySettingsFormProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const feedback = useActionFeedback<"saved">();
  const update = useUpdateSecuritySettings();
  const schema = useMemo(() => securitySchema(t), [t]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors, isDirty },
  } = useForm<SecurityValues>({ resolver: zodResolver(schema), defaultValues: valuesOf(settings) });
  const trustedEnabled = useWatch({ control, name: "trustedDevicesEnabled" });

  function submit(values: SecurityValues) {
    if (update.isPending) {
      return;
    }
    const patch: SecurityPatch = changedKeys<SecuritySettings>(settings, candidateOf(values));
    if (Object.keys(patch).length === 0) {
      return;
    }
    feedback.clear();
    update.mutate(patch, {
      onSuccess: (saved) => {
        reset(valuesOf(saved));
        feedback.succeed("saved");
      },
      onError: (error) => {
        const failure = mapSettingsError(error, t, FIELD_KEYS);
        if (failure === null) {
          return;
        }
        if (failure.kind === "general") {
          feedback.fail(failure.error);
          return;
        }
        const field = failure.key as keyof SecurityValues;
        setError(field, { type: "server", message: failure.message });
        setFocus(field);
      },
    });
  }

  const pending = update.isPending;

  const numeric = (field: NumericField) => (
    <div key={field.key} style={{ flex: "1 1 220px", minWidth: 0 }}>
      <FormField
        id={id(field.key)}
        label={t(`settings.security.fields.${field.key}`)}
        error={errors[field.key]?.message}
        extra={t(`settings.security.hints.${field.key}`)}
      >
        {(control_) => (
          <Controller
            name={field.key}
            control={control}
            render={({ field: input }) => (
              <InputNumber
                {...control_}
                precision={0}
                value={input.value}
                onChange={input.onChange}
                suffix={t(`units.${field.unit}`)}
                style={{ width: "100%" }}
              />
            )}
          />
        )}
      </FormField>
    </div>
  );

  const flag = (key: FlagKey) => (
    <Controller
      name={key}
      control={control}
      render={({ field }) => (
        <Flex vertical gap={2} style={{ marginBottom: token.marginSM }}>
          <Flex align="center" gap={token.marginXS}>
            <Switch
              id={id(key)}
              checked={field.value}
              onChange={field.onChange}
              disabled={pending}
            />
            <label htmlFor={id(key)}>{t(`settings.security.fields.${key}`)}</label>
          </Flex>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t(`settings.security.hints.${key}`)}
          </Typography.Text>
        </Flex>
      )}
    />
  );

  return (
    <form
      noValidate
      aria-busy={pending}
      onSubmit={(event) => {
        void handleSubmit(submit)(event);
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false} disabled={pending}>
        <Flex vertical gap={token.marginXXS}>
          <FeedbackAlerts feedback={feedback} noticeKey={() => "settings.saved"} />
          {GROUPS.map((group) => (
            <div key={group.key}>
              <Divider titlePlacement="start" style={{ marginBlock: token.marginSM }}>
                {t(`settings.security.groups.${group.key}`)}
              </Divider>
              <Flex gap={token.marginSM} wrap>
                {group.fields.map(numeric)}
              </Flex>
            </div>
          ))}
          <Divider titlePlacement="start" style={{ marginBlock: token.marginSM }}>
            {t("settings.security.groups.twoFactor")}
          </Divider>
          {flag("twoFactorRequired")}
          <Divider titlePlacement="start" style={{ marginBlock: token.marginSM }}>
            {t("settings.security.groups.trustedDevices")}
          </Divider>
          {flag("trustedDevicesEnabled")}
          {trustedEnabled ? (
            <Flex gap={token.marginSM}>{numeric(TRUSTED_DEVICE_DURATION)}</Flex>
          ) : null}
          <div style={{ marginTop: token.marginSM }}>
            <Button type="primary" htmlType="submit" loading={pending} disabled={!isDirty}>
              {t("settings.save")}
            </Button>
          </div>
        </Flex>
      </Form>
    </form>
  );
}
