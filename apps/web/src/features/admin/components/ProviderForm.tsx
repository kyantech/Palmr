import { zodResolver } from "@hookform/resolvers/zod";
import {
  Alert,
  Button,
  Checkbox,
  Collapse,
  Flex,
  Form,
  Input,
  Select,
  Skeleton,
  Switch,
  Tag,
  theme,
  Typography,
} from "antd";
import { useId, useMemo, useState } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { ApiError, detailFields, ErrorAlert } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useCreateProvider, useDiscoverProvider, useUpdateProvider } from "../api/mutations";
import { useProviderPresets } from "../api/queries";
import { type Provider, TOKEN_AUTH_METHODS } from "../types";
import { DiscoveryPreview } from "./DiscoveryPreview";
import { reportable } from "./feedback";
import {
  applyDiscovery,
  createBody,
  emptyValues,
  FORM_FIELD_NAMES,
  type FormMode,
  presetKeyOf,
  type ProviderFormValues,
  providerSchema,
  updateBody,
  valuesFromPreset,
  valuesFromProvider,
} from "./providerFormModel";
import { RedirectUri } from "./RedirectUri";

interface ProviderFormProps {
  mode: FormMode;
  provider: Provider | null;
  onClose: () => void;
}

const FLAG_TEST_IDS = {
  autoProvision: "provider-flag-auto-provision",
  allowEmailLinking: "provider-flag-email-linking",
  enabled: "provider-flag-enabled",
} as const;

const CLAIM_FIELDS = [
  ["claimSubject", "subject"],
  ["claimEmail", "email"],
  ["claimEmailVerified", "emailVerified"],
  ["claimUsername", "username"],
  ["claimName", "name"],
  ["claimPicture", "picture"],
] as const;

const FIELD_TARGETS: Readonly<Record<(typeof FORM_FIELD_NAMES)[number], keyof ProviderFormValues>> =
  {
    slug: "slug",
    displayName: "displayName",
    issuerUrl: "issuerUrl",
    clientId: "clientId",
    clientSecret: "clientSecret",
    tokenAuthMethod: "tokenAuthMethod",
    scopes: "scopes",
    endpoints: "authorizationEndpoint",
    claimMapping: "claimSubject",
  };

export function ProviderForm({ mode, provider, onClose }: ProviderFormProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const presets = useProviderPresets(mode === "create");
  const create = useCreateProvider();
  const update = useUpdateProvider();
  const discover = useDiscoverProvider();
  const [failure, setFailure] = useState<unknown>(null);
  const schema = useMemo(() => providerSchema(t, mode), [t, mode]);
  const {
    control,
    handleSubmit,
    getValues,
    reset,
    setValue,
    setError,
    setFocus,
    formState: { errors, isDirty },
  } = useForm<ProviderFormValues>({
    resolver: zodResolver(schema),
    defaultValues: provider === null ? emptyValues() : valuesFromProvider(provider),
  });
  const protocol = useWatch({ control, name: "protocol" });
  const presetKey = useWatch({ control, name: "presetKey" });
  const tokenAuthMethod = useWatch({ control, name: "tokenAuthMethod" });
  const pending = create.isPending || update.isPending;
  const chosen = mode === "edit" || presetKey !== "";

  function markServerFields(error: unknown): boolean {
    if (!(error instanceof ApiError)) {
      return false;
    }
    if (error.code === "PROVIDER_SLUG_TAKEN") {
      setError("slug", { type: "server", message: t("providers.form.validation.slugTaken") });
      setFocus("slug");
      return true;
    }
    if (error.code !== "VALIDATION_ERROR") {
      return false;
    }
    let marked = false;
    for (const name of detailFields(error)) {
      const target = (FIELD_TARGETS as Readonly<Record<string, keyof ProviderFormValues>>)[name];
      if (target !== undefined) {
        setError(target, { type: "server", message: t("providers.form.validation.rejected") });
        marked = true;
      }
    }
    return marked;
  }

  function fail(error: unknown) {
    const shown = reportable(error);
    if (shown === null) {
      return;
    }
    setFailure(markServerFields(shown) ? null : shown);
  }

  function submit(values: ProviderFormValues) {
    if (pending) {
      return;
    }
    setFailure(null);
    if (mode === "create") {
      create.mutate(createBody(values), { onSuccess: onClose, onError: fail });
      return;
    }
    if (provider === null) {
      return;
    }
    const body = updateBody(provider, values);
    if (Object.keys(body).length === 0) {
      onClose();
      return;
    }
    update.mutate({ id: provider.id, body }, { onSuccess: onClose, onError: fail });
  }

  function choosePreset(key: string) {
    const item = presets.data?.items.find((entry) => presetKeyOf(entry) === key);
    if (item !== undefined) {
      setFailure(null);
      discover.reset();
      reset(valuesFromPreset(item));
    }
  }

  function runDiscovery() {
    const issuer = getValues("issuerUrl").trim();
    if (issuer === "") {
      setError("issuerUrl", { type: "required", message: t("validation.required") });
      setFocus("issuerUrl");
      return;
    }
    discover.mutate(issuer);
  }

  function applyDiscovered() {
    if (discover.data === undefined) {
      return;
    }
    const applied = applyDiscovery(getValues(), discover.data);
    for (const key of [
      "issuerUrl",
      "authorizationEndpoint",
      "tokenEndpoint",
      "userinfoEndpoint",
      "jwksEndpoint",
    ] as const) {
      setValue(key, applied[key], { shouldDirty: true, shouldValidate: true });
    }
  }

  const text = (
    name: keyof ProviderFormValues,
    label: string,
    options: { extra?: string; autoComplete?: string; readOnly?: boolean } = {},
  ) => (
    <FormField
      id={id(name)}
      label={label}
      error={errors[name]?.message}
      {...(options.extra === undefined ? {} : { extra: options.extra })}
    >
      {(aria) => (
        <Controller
          name={name}
          control={control}
          render={({ field }) => (
            <Input
              {...aria}
              name={field.name}
              ref={field.ref}
              onBlur={field.onBlur}
              onChange={field.onChange}
              value={field.value as string}
              disabled={pending}
              readOnly={options.readOnly ?? false}
              autoComplete={options.autoComplete ?? "off"}
              spellCheck={false}
            />
          )}
        />
      )}
    </FormField>
  );

  const flag = (name: keyof typeof FLAG_TEST_IDS) => (
    <Controller
      name={name}
      control={control}
      render={({ field }) => (
        <Flex vertical gap={2} style={{ marginBottom: token.marginSM }}>
          <Flex align="center" gap={token.marginXS}>
            <Switch
              id={id(name)}
              data-testid={FLAG_TEST_IDS[name]}
              checked={field.value}
              onChange={field.onChange}
              disabled={pending}
            />
            <label htmlFor={id(name)}>{t(`providers.form.flags.${name}.label`)}</label>
          </Flex>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t(`providers.form.flags.${name}.hint`)}
          </Typography.Text>
        </Flex>
      )}
    />
  );

  const endpointFields = (
    <Flex vertical>
      {text("authorizationEndpoint", t("providers.form.endpoints.authorization"))}
      {text("tokenEndpoint", t("providers.form.endpoints.token"))}
      {text("userinfoEndpoint", t("providers.form.endpoints.userinfo"))}
      {protocol === "oidc" ? text("jwksEndpoint", t("providers.form.endpoints.jwks")) : null}
    </Flex>
  );

  const claimFields = (
    <Flex gap={token.marginSM} wrap>
      {CLAIM_FIELDS.map(([name, claim]) => (
        <div key={name} style={{ flex: "1 1 200px", minWidth: 0 }}>
          {text(name, t(`providers.form.claims.${claim}`))}
        </div>
      ))}
    </Flex>
  );

  const presetOptions = (presets.data?.items ?? []).map((item) => ({
    value: presetKeyOf(item),
    label:
      item.preset === "generic"
        ? t(`providers.form.presets.custom.${item.protocol}`)
        : item.displayName,
  }));

  return (
    <form
      noValidate
      aria-busy={pending}
      data-testid="provider-form"
      onSubmit={(event) => {
        void handleSubmit(submit)(event);
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false} disabled={pending}>
        <Flex vertical>
          {failure === null ? null : <ErrorAlert error={failure} />}
          {mode === "create" ? (
            presets.isPending ? (
              <Skeleton active paragraph={{ rows: 1 }} />
            ) : presets.isError ? (
              <ErrorAlert error={presets.error} />
            ) : (
              <FormField
                id={id("presetKey")}
                label={t("providers.form.preset")}
                error={errors.presetKey?.message}
                extra={t("providers.form.presetHint")}
              >
                {(aria) => (
                  <Select
                    {...aria}
                    value={presetKey === "" ? null : presetKey}
                    options={presetOptions}
                    placeholder={t("providers.form.presetPlaceholder")}
                    onChange={choosePreset}
                    disabled={pending}
                  />
                )}
              </FormField>
            )
          ) : (
            <Flex gap={token.marginXS} align="center" style={{ marginBottom: token.marginSM }}>
              <Tag variant="filled">{t(`providers.row.protocol.${protocol}`)}</Tag>
              <Typography.Text type="secondary" code>
                {provider?.slug}
              </Typography.Text>
            </Flex>
          )}
          {chosen ? (
            <>
              {text("displayName", t("providers.form.displayName"))}
              {mode === "create"
                ? text("slug", t("providers.form.slug"), { extra: t("providers.form.slugHint") })
                : null}
              {protocol === "oidc" ? (
                <>
                  {text("issuerUrl", t("providers.form.issuerUrl"), {
                    extra: t("providers.form.issuerHint"),
                  })}
                  <div style={{ marginBottom: token.marginSM }}>
                    <Button loading={discover.isPending} onClick={runDiscovery}>
                      {t("providers.discovery.run")}
                    </Button>
                  </div>
                  {discover.isError ? (
                    <div style={{ marginBottom: token.marginSM }}>
                      <ErrorAlert error={discover.error} />
                    </div>
                  ) : null}
                  {discover.isSuccess ? (
                    <DiscoveryPreview discovered={discover.data} onApply={applyDiscovered} />
                  ) : null}
                </>
              ) : null}
              {text("clientId", t("providers.form.clientId"))}
              <FormField
                id={id("clientSecret")}
                label={t("providers.form.clientSecret")}
                error={errors.clientSecret?.message}
                extra={
                  mode === "edit"
                    ? t("providers.form.secretKeepHint")
                    : t("providers.form.secretWriteOnly")
                }
              >
                {(aria) => (
                  <Controller
                    name="clientSecret"
                    control={control}
                    render={({ field }) => (
                      <Input.Password
                        {...aria}
                        name={field.name}
                        ref={field.ref}
                        onBlur={field.onBlur}
                        onChange={field.onChange}
                        value={field.value}
                        disabled={pending}
                        autoComplete="new-password"
                        visibilityToggle={false}
                        placeholder={
                          provider?.clientSecretConfigured === true
                            ? t("providers.form.secretPlaceholderConfigured")
                            : undefined
                        }
                      />
                    )}
                  />
                )}
              </FormField>
              {mode === "edit" ? (
                <Flex align="center" gap={token.marginXS} style={{ marginBottom: token.marginSM }}>
                  <Tag
                    color={provider?.clientSecretConfigured === true ? "success" : "default"}
                    variant="filled"
                    data-testid="provider-form-secret-state"
                    data-configured={provider?.clientSecretConfigured === true ? "true" : "false"}
                  >
                    {t(
                      provider?.clientSecretConfigured === true
                        ? "providers.row.secret.configured"
                        : "providers.row.secret.missing",
                    )}
                  </Tag>
                </Flex>
              ) : null}
              <FormField
                id={id("tokenAuthMethod")}
                label={t("providers.form.tokenAuthMethod")}
                error={errors.tokenAuthMethod?.message}
              >
                {(aria) => (
                  <Controller
                    name="tokenAuthMethod"
                    control={control}
                    render={({ field }) => (
                      <Select
                        {...aria}
                        value={field.value}
                        onChange={field.onChange}
                        disabled={pending}
                        options={TOKEN_AUTH_METHODS.map((method) => ({
                          value: method,
                          label: t(`providers.form.tokenMethods.${method}`),
                        }))}
                      />
                    )}
                  />
                )}
              </FormField>
              {mode === "edit" &&
              tokenAuthMethod === "none" &&
              provider?.clientSecretConfigured === true ? (
                <Controller
                  name="clearSecret"
                  control={control}
                  render={({ field }) => (
                    <Checkbox
                      checked={field.value}
                      onChange={field.onChange}
                      disabled={pending}
                      style={{ marginBottom: token.marginSM }}
                    >
                      {t("providers.form.clearSecret")}
                    </Checkbox>
                  )}
                />
              ) : null}
              <FormField
                id={id("scopes")}
                label={t("providers.form.scopes")}
                error={errors.scopes?.message}
                extra={t("providers.form.scopesHint")}
              >
                {(aria) => (
                  <Controller
                    name="scopes"
                    control={control}
                    render={({ field }) => (
                      <Select
                        {...aria}
                        mode="tags"
                        value={field.value}
                        onChange={field.onChange}
                        tokenSeparators={[" ", ","]}
                        open={false}
                        disabled={pending}
                      />
                    )}
                  />
                )}
              </FormField>
              {protocol === "oauth2" ? (
                <Flex vertical data-testid="provider-form-endpoints">
                  <Typography.Text strong style={{ marginBottom: token.marginXS }}>
                    {t("providers.form.endpoints.title")}
                  </Typography.Text>
                  {endpointFields}
                </Flex>
              ) : null}
              <Collapse
                ghost
                items={[
                  ...(protocol === "oidc"
                    ? [
                        {
                          key: "endpoints",
                          label: t("providers.form.endpoints.advanced"),
                          forceRender: true,
                          children: endpointFields,
                        },
                      ]
                    : []),
                  {
                    key: "claims",
                    label: t("providers.form.claims.title"),
                    forceRender: true,
                    children: claimFields,
                  },
                ]}
              />
              <Flex vertical style={{ marginTop: token.marginSM }}>
                {flag("autoProvision")}
                {flag("allowEmailLinking")}
                {mode === "create" ? flag("enabled") : null}
                <Alert
                  type="info"
                  showIcon
                  style={{ marginBottom: token.marginSM }}
                  title={t("providers.form.consequences.title")}
                  description={
                    <Flex vertical gap={2}>
                      <span>{t("providers.form.consequences.autoProvision")}</span>
                      <span>{t("providers.form.consequences.emailLinking")}</span>
                      <span>{t("providers.form.consequences.manualLinking")}</span>
                    </Flex>
                  }
                />
              </Flex>
              {provider === null ? (
                <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
                  {t("providers.form.redirectAfterCreate")}
                </Typography.Text>
              ) : (
                <RedirectUri value={provider.redirectUri} />
              )}
            </>
          ) : null}
          <Flex justify="end" gap={token.marginXS} style={{ marginTop: token.marginLG }}>
            <Button onClick={onClose} disabled={pending}>
              {t("common.cancel")}
            </Button>
            <Button
              type="primary"
              htmlType="submit"
              loading={pending}
              disabled={!chosen || (mode === "edit" && !isDirty)}
            >
              {mode === "create" ? t("providers.form.create") : t("providers.form.save")}
            </Button>
          </Flex>
        </Flex>
      </Form>
    </form>
  );
}
