import { zodResolver } from "@hookform/resolvers/zod";
import {
  Alert,
  Button,
  Flex,
  Form,
  Input,
  InputNumber,
  Select,
  Switch,
  Tag,
  theme,
  Typography,
} from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo, useState } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ApiError, ErrorAlert } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useSmtpTest, useUpdateSmtpSettings } from "../api/mutations";
import {
  SMTP_SECURITY_MODES,
  type SmtpPatch,
  type SmtpSecurity,
  type SmtpSettings,
  type SmtpTestResult,
  type SmtpUnsavedSettings,
} from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { ConfirmDialog } from "./ConfirmDialog";
import { detailText } from "./feedback";
import { requiredInteger } from "./fieldSchemas";
import { Section } from "./Section";
import { mapSettingsError } from "./settingsErrors";

interface SmtpValues {
  enabled: boolean;
  host: string;
  port: number | null;
  security: SmtpSecurity;
  username: string;
  noAuth: boolean;
  password: string;
  clearPassword: boolean;
  fromName: string;
  fromEmail: string;
  allowSelfSignedCertificate: boolean;
}

type SmtpField = keyof SmtpValues;

const SETTING_KEYS: readonly string[] = [
  "enabled",
  "host",
  "port",
  "security",
  "username",
  "noAuth",
  "password",
  "fromName",
  "fromEmail",
  "allowSelfSignedCertificate",
];

const STAGE_NAMES = ["connect", "starttls", "auth", "send"] as const;

function valuesOf(settings: SmtpSettings): SmtpValues {
  return {
    enabled: settings.enabled,
    host: settings.host ?? "",
    port: settings.port,
    security: settings.security,
    username: settings.username ?? "",
    noAuth: settings.noAuth,
    password: "",
    clearPassword: false,
    fromName: settings.fromName ?? "",
    fromEmail: settings.fromEmail ?? "",
    allowSelfSignedCertificate: settings.allowSelfSignedCertificate,
  };
}

function textOrNull(value: string): string | null {
  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed;
}

export function smtpPatchOf(current: SmtpSettings, values: SmtpValues): SmtpPatch {
  const patch: SmtpPatch = {};
  if (values.enabled !== current.enabled) {
    patch.enabled = values.enabled;
  }
  const host = textOrNull(values.host);
  if (host !== current.host) {
    patch.host = host;
  }
  if (values.port !== null && values.port !== current.port) {
    patch.port = values.port;
  }
  if (values.security !== current.security) {
    patch.security = values.security;
  }
  const username = textOrNull(values.username);
  if (username !== current.username) {
    patch.username = username;
  }
  if (values.noAuth !== current.noAuth) {
    patch.noAuth = values.noAuth;
  }
  const fromName = textOrNull(values.fromName);
  if (fromName !== current.fromName) {
    patch.fromName = fromName;
  }
  const fromEmail = textOrNull(values.fromEmail);
  if (fromEmail !== current.fromEmail) {
    patch.fromEmail = fromEmail;
  }
  if (values.allowSelfSignedCertificate !== current.allowSelfSignedCertificate) {
    patch.allowSelfSignedCertificate = values.allowSelfSignedCertificate;
  }
  if (values.password !== "") {
    patch.password = values.password;
  } else if (values.clearPassword) {
    patch.password = null;
  }
  return patch;
}

export function unsavedSmtpOf(values: SmtpValues): SmtpUnsavedSettings | null {
  const host = textOrNull(values.host);
  const fromEmail = textOrNull(values.fromEmail);
  if (host === null || fromEmail === null || values.port === null) {
    return null;
  }
  const username = textOrNull(values.username);
  const fromName = textOrNull(values.fromName);
  return {
    host,
    port: values.port,
    security: values.security,
    fromEmail,
    allowSelfSignedCertificate: values.allowSelfSignedCertificate,
    noAuth: values.noAuth,
    ...(fromName === null ? {} : { fromName }),
    ...(username === null || values.noAuth ? {} : { username }),
    ...(values.password === "" || values.noAuth ? {} : { password: values.password }),
  };
}

function smtpSchema(t: TFunction<"admin">) {
  return z.object({
    enabled: z.boolean(),
    host: z.string(),
    port: requiredInteger(t),
    security: z.enum(SMTP_SECURITY_MODES),
    username: z.string(),
    noAuth: z.boolean(),
    password: z.string(),
    clearPassword: z.boolean(),
    fromName: z.string(),
    fromEmail: z.string(),
    allowSelfSignedCertificate: z.boolean(),
  });
}

type Outcome =
  | { kind: "result"; mode: "saved" | "unsaved"; result: SmtpTestResult }
  | { kind: "failure"; mode: "saved" | "unsaved"; error: unknown };

export function SmtpWorkspace({ settings }: { settings: SmtpSettings }) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const feedback = useActionFeedback<"saved">();
  const update = useUpdateSmtpSettings();
  const test = useSmtpTest();
  const schema = useMemo(() => smtpSchema(t), [t]);
  const [confirmingClear, setConfirmingClear] = useState(false);
  const [recipient, setRecipient] = useState("");
  const [recipientError, setRecipientError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    setValue,
    getValues,
    formState: { errors, isDirty },
  } = useForm<SmtpValues>({ resolver: zodResolver(schema), defaultValues: valuesOf(settings) });
  const noAuth = useWatch({ control, name: "noAuth" });
  const clearPassword = useWatch({ control, name: "clearPassword" });
  const typedPassword = useWatch({ control, name: "password" }) !== "";

  function submit(values: SmtpValues) {
    if (update.isPending) {
      return;
    }
    const patch = smtpPatchOf(settings, values);
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
        const failure = mapSettingsError(error, t, SETTING_KEYS);
        if (failure === null) {
          return;
        }
        if (failure.kind === "general") {
          feedback.fail(failure.error);
          return;
        }
        const field = failure.key as SmtpField;
        setError(field, { type: "server", message: failure.message });
        setFocus(field);
      },
    });
  }

  function runTest(mode: "saved" | "unsaved") {
    if (test.isPending) {
      return;
    }
    const to = recipient.trim();
    if (to === "") {
      setRecipientError(t("validation.required"));
      return;
    }
    setRecipientError(null);
    let body;
    if (mode === "saved") {
      body = { to };
    } else {
      const unsaved = unsavedSmtpOf(getValues());
      if (unsaved === null) {
        setRecipientError(t("smtp.test.unsavedIncomplete"));
        return;
      }
      body = { to, useUnsavedSettings: unsaved };
    }
    setOutcome(null);
    test.mutate(body, {
      onSuccess: (result) => {
        setOutcome({ kind: "result", mode, result });
      },
      onError: (error) => {
        if (error instanceof ApiError && error.code === "AUTH_RECENT_AUTH_REQUIRED") {
          return;
        }
        setOutcome({ kind: "failure", mode, error });
      },
    });
  }

  const pending = update.isPending;

  return (
    <Flex vertical gap={token.margin}>
      <Section
        title={t("smtp.title")}
        description={t("smtp.description")}
        testId="smtp-settings"
        extra={
          <Tag
            color={settings.enabled ? "success" : "default"}
            variant="filled"
            style={{ marginInlineEnd: 0 }}
          >
            {t(settings.enabled ? "smtp.enabledTag" : "smtp.disabledTag")}
          </Tag>
        }
      >
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
              <Controller
                name="enabled"
                control={control}
                render={({ field }) => (
                  <Flex
                    align="center"
                    gap={token.marginXS}
                    style={{ marginBottom: token.marginSM }}
                  >
                    <Switch
                      id={id("enabled")}
                      checked={field.value}
                      onChange={field.onChange}
                      disabled={pending}
                    />
                    <label htmlFor={id("enabled")}>{t("smtp.fields.enabled")}</label>
                  </Flex>
                )}
              />
              <Flex gap={token.marginSM} wrap>
                <div style={{ flex: "2 1 260px" }}>
                  <FormField
                    id={id("host")}
                    label={t("smtp.fields.host")}
                    error={errors.host?.message}
                  >
                    {(control_) => (
                      <Controller
                        name="host"
                        control={control}
                        render={({ field }) => (
                          <Input {...field} {...control_} autoComplete="off" spellCheck={false} />
                        )}
                      />
                    )}
                  </FormField>
                </div>
                <div style={{ flex: "1 1 120px" }}>
                  <FormField
                    id={id("port")}
                    label={t("smtp.fields.port")}
                    error={errors.port?.message}
                  >
                    {(control_) => (
                      <Controller
                        name="port"
                        control={control}
                        render={({ field }) => (
                          <InputNumber
                            {...control_}
                            precision={0}
                            value={field.value}
                            onChange={field.onChange}
                            style={{ width: "100%" }}
                          />
                        )}
                      />
                    )}
                  </FormField>
                </div>
                <div style={{ flex: "1 1 180px" }}>
                  <FormField
                    id={id("security")}
                    label={t("smtp.fields.security")}
                    error={errors.security?.message}
                  >
                    {(control_) => (
                      <Controller
                        name="security"
                        control={control}
                        render={({ field }) => (
                          <Select<SmtpSecurity>
                            {...control_}
                            value={field.value}
                            onChange={field.onChange}
                            options={SMTP_SECURITY_MODES.map((mode) => ({
                              value: mode,
                              label: t(`smtp.security.${mode}`),
                            }))}
                          />
                        )}
                      />
                    )}
                  </FormField>
                </div>
              </Flex>
              <Controller
                name="noAuth"
                control={control}
                render={({ field }) => (
                  <Flex
                    align="center"
                    gap={token.marginXS}
                    style={{ marginBottom: token.marginSM }}
                  >
                    <Switch
                      id={id("noAuth")}
                      checked={field.value}
                      onChange={field.onChange}
                      disabled={pending}
                    />
                    <label htmlFor={id("noAuth")}>{t("smtp.fields.noAuth")}</label>
                  </Flex>
                )}
              />
              <Flex gap={token.marginSM} wrap>
                <div style={{ flex: "1 1 240px" }}>
                  <FormField
                    id={id("username")}
                    label={t("smtp.fields.username")}
                    error={errors.username?.message}
                  >
                    {(control_) => (
                      <Controller
                        name="username"
                        control={control}
                        render={({ field }) => (
                          <Input
                            {...field}
                            {...control_}
                            disabled={noAuth || pending}
                            autoComplete="off"
                          />
                        )}
                      />
                    )}
                  </FormField>
                </div>
                <div style={{ flex: "1 1 240px" }}>
                  <FormField
                    id={id("password")}
                    label={t("smtp.fields.password")}
                    error={errors.password?.message}
                  >
                    {(control_) => (
                      <Controller
                        name="password"
                        control={control}
                        render={({ field }) => (
                          <Input.Password
                            {...field}
                            {...control_}
                            disabled={noAuth || pending}
                            autoComplete="new-password"
                            placeholder={t("smtp.fields.passwordPlaceholder")}
                            onChange={(event) => {
                              field.onChange(event);
                              if (event.target.value !== "") {
                                setValue("clearPassword", false, { shouldDirty: true });
                              }
                            }}
                          />
                        )}
                      />
                    )}
                  </FormField>
                  <Flex
                    align="center"
                    gap={token.marginXS}
                    wrap
                    style={{ marginBottom: token.marginSM }}
                  >
                    {settings.passwordConfigured ? (
                      <Tag
                        color={clearPassword ? "warning" : "success"}
                        variant="filled"
                        style={{ marginInlineEnd: 0 }}
                        data-testid="smtp-password-state"
                      >
                        {t(
                          clearPassword
                            ? "smtp.password.willClear"
                            : typedPassword
                              ? "smtp.password.willReplace"
                              : "smtp.password.configured",
                        )}
                      </Tag>
                    ) : (
                      <Tag
                        variant="filled"
                        style={{ marginInlineEnd: 0 }}
                        data-testid="smtp-password-state"
                      >
                        {t("smtp.password.notConfigured")}
                      </Tag>
                    )}
                    {settings.passwordConfigured && !clearPassword ? (
                      <Button
                        type="link"
                        size="small"
                        danger
                        disabled={pending}
                        onClick={() => {
                          setConfirmingClear(true);
                        }}
                      >
                        {t("smtp.password.clear")}
                      </Button>
                    ) : null}
                    {clearPassword ? (
                      <Button
                        type="link"
                        size="small"
                        disabled={pending}
                        onClick={() => {
                          setValue("clearPassword", false, { shouldDirty: true });
                        }}
                      >
                        {t("smtp.password.undoClear")}
                      </Button>
                    ) : null}
                  </Flex>
                </div>
              </Flex>
              <Flex gap={token.marginSM} wrap>
                <div style={{ flex: "1 1 240px" }}>
                  <FormField
                    id={id("fromName")}
                    label={t("smtp.fields.fromName")}
                    error={errors.fromName?.message}
                  >
                    {(control_) => (
                      <Controller
                        name="fromName"
                        control={control}
                        render={({ field }) => <Input {...field} {...control_} />}
                      />
                    )}
                  </FormField>
                </div>
                <div style={{ flex: "1 1 240px" }}>
                  <FormField
                    id={id("fromEmail")}
                    label={t("smtp.fields.fromEmail")}
                    error={errors.fromEmail?.message}
                  >
                    {(control_) => (
                      <Controller
                        name="fromEmail"
                        control={control}
                        render={({ field }) => (
                          <Input {...field} {...control_} type="email" autoComplete="off" />
                        )}
                      />
                    )}
                  </FormField>
                </div>
              </Flex>
              <Controller
                name="allowSelfSignedCertificate"
                control={control}
                render={({ field }) => (
                  <Flex vertical gap={2} style={{ marginBottom: token.marginSM }}>
                    <Flex align="center" gap={token.marginXS}>
                      <Switch
                        id={id("allowSelfSigned")}
                        checked={field.value}
                        onChange={field.onChange}
                        disabled={pending}
                      />
                      <label htmlFor={id("allowSelfSigned")}>
                        {t("smtp.fields.allowSelfSigned")}
                      </label>
                    </Flex>
                    <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
                      {t("smtp.fields.allowSelfSignedHint")}
                    </Typography.Text>
                  </Flex>
                )}
              />
              <div>
                <Button type="primary" htmlType="submit" loading={pending} disabled={!isDirty}>
                  {t("settings.save")}
                </Button>
              </div>
            </Flex>
          </Form>
        </form>
      </Section>
      <Section
        title={t("smtp.test.title")}
        description={t("smtp.test.description")}
        testId="smtp-test"
      >
        <Flex vertical gap={token.marginSM} style={{ maxWidth: 520 }}>
          <FormField
            id={id("recipient")}
            label={t("smtp.test.recipient")}
            error={recipientError ?? undefined}
            style={{ marginBottom: 0 }}
          >
            {(control_) => (
              <Input
                {...control_}
                type="email"
                value={recipient}
                autoComplete="off"
                onChange={(event) => {
                  setRecipient(event.target.value);
                }}
              />
            )}
          </FormField>
          <Flex gap={token.marginXS} wrap>
            <Button
              loading={test.isPending}
              onClick={() => {
                runTest("saved");
              }}
            >
              {t("smtp.test.saved")}
            </Button>
            <Button
              loading={test.isPending}
              onClick={() => {
                runTest("unsaved");
              }}
            >
              {t("smtp.test.unsaved")}
            </Button>
          </Flex>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t("smtp.test.unsavedHint")}
          </Typography.Text>
          {outcome === null ? null : <TestOutcome outcome={outcome} />}
        </Flex>
      </Section>
      <ConfirmDialog
        open={confirmingClear}
        title={t("smtp.password.clearDialog.title")}
        description={
          <Typography.Text>{t("smtp.password.clearDialog.description")}</Typography.Text>
        }
        confirmLabel={t("smtp.password.clearDialog.confirm")}
        danger
        onCancel={() => {
          setConfirmingClear(false);
        }}
        onConfirm={() => {
          setValue("password", "", { shouldDirty: true });
          setValue("clearPassword", true, { shouldDirty: true });
          setConfirmingClear(false);
        }}
      />
    </Flex>
  );
}

function TestOutcome({ outcome }: { outcome: Outcome }) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const modeLabel = t(`smtp.test.mode.${outcome.mode}`);
  if (outcome.kind === "failure") {
    const stage = outcome.error instanceof ApiError ? detailText(outcome.error, "stage") : null;
    const known = STAGE_NAMES.find((name) => name === stage);
    return (
      <Flex vertical gap={token.marginXS} data-testid="smtp-test-failure">
        <ErrorAlert error={outcome.error} />
        {known === undefined ? null : (
          <Typography.Text type="secondary">
            {t("smtp.test.failedAt", { stage: t(`smtp.test.stages.${known}`), mode: modeLabel })}
          </Typography.Text>
        )}
      </Flex>
    );
  }
  const { result } = outcome;
  return (
    <Flex vertical gap={token.marginXS} data-testid="smtp-test-result">
      <Alert
        type={result.ok ? "success" : "warning"}
        showIcon
        role="status"
        title={t(result.ok ? "smtp.test.succeeded" : "smtp.test.incomplete", { mode: modeLabel })}
        description={t("smtp.test.duration", { ms: result.durationMs })}
      />
      <ol aria-label={t("smtp.test.stagesLabel")} style={{ margin: 0, paddingInlineStart: 0 }}>
        {result.stages.map((stage) => (
          <li
            key={stage.name}
            data-stage={stage.name}
            data-ok={stage.ok ? "true" : "false"}
            style={{
              listStyle: "none",
              display: "flex",
              gap: token.marginXS,
              alignItems: "center",
            }}
          >
            <Tag
              color={stage.ok ? "success" : "error"}
              variant="filled"
              style={{ marginInlineEnd: 0 }}
            >
              {t(stage.ok ? "smtp.test.stageOk" : "smtp.test.stageFailed")}
            </Tag>
            <Typography.Text>{t(`smtp.test.stages.${stage.name}`)}</Typography.Text>
          </li>
        ))}
      </ol>
    </Flex>
  );
}
