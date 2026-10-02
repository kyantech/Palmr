import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, Modal, Radio, Select, Switch, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo, useRef, useState } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ApiError, ErrorAlert } from "../../../shared/errors";
import { localeOptions } from "../../../shared/format/locale";
import { FormField } from "../../../shared/ui/FormField";
import { useCreateUser } from "../api/mutations";
import { USER_ROLES, type CreateUserRequest, type UserRole, type UserRow } from "../types";
import { BytesInput, type BytesValue, bytesOf, isWithinByteRange } from "./BytesInput";
import { detailNumber, invalidFields, newIdempotencyKey, reportable } from "./feedback";

const INSTANCE_LOCALE = "";
const FIELDS = [
  "firstName",
  "lastName",
  "username",
  "email",
  "role",
  "password",
  "locale",
] as const;

interface CreateValues {
  firstName: string;
  lastName: string;
  username: string;
  email: string;
  role: UserRole;
  locale: string;
  password: string;
  requirePasswordChange: boolean;
  isActive: boolean;
  quotaMode: "inherit" | "custom";
  quota: BytesValue;
}

const DEFAULTS: CreateValues = {
  firstName: "",
  lastName: "",
  username: "",
  email: "",
  role: "user",
  locale: INSTANCE_LOCALE,
  password: "",
  requirePasswordChange: true,
  isActive: true,
  quotaMode: "inherit",
  quota: { amount: null, unit: "GiB" },
};

function createSchema(t: TFunction<"admin">) {
  const required = t("validation.required");
  return z
    .object({
      firstName: z.string().trim().min(1, required),
      lastName: z.string().trim().min(1, required),
      username: z.string().trim().min(1, required),
      email: z
        .string()
        .trim()
        .min(1, required)
        .pipe(z.email(t("validation.email"))),
      role: z.enum(USER_ROLES),
      locale: z.string(),
      password: z.string(),
      requirePasswordChange: z.boolean(),
      isActive: z.boolean(),
      quotaMode: z.enum(["inherit", "custom"]),
      quota: z.object({
        amount: z.number().nullable(),
        unit: z.enum(["B", "KiB", "MiB", "GiB", "TiB", "PiB"]),
      }),
    })
    .superRefine((values, context) => {
      if (values.quotaMode === "custom" && !isWithinByteRange(values.quota)) {
        context.addIssue({
          code: "custom",
          path: ["quota"],
          message: t("validation.quota"),
        });
      }
    });
}

function requestOf(values: CreateValues): CreateUserRequest {
  const quotaBytes = values.quotaMode === "custom" ? bytesOf(values.quota) : null;
  return {
    firstName: values.firstName.trim(),
    lastName: values.lastName.trim(),
    username: values.username.trim(),
    email: values.email.trim(),
    role: values.role,
    isActive: values.isActive,
    ...(values.locale === INSTANCE_LOCALE ? {} : { locale: values.locale }),
    ...(values.password === ""
      ? {}
      : { password: values.password, requirePasswordChange: values.requirePasswordChange }),
    ...(quotaBytes === null ? {} : { quotaBytes }),
  };
}

interface CreateUserModalProps {
  open: boolean;
  locales: readonly string[];
  onClose: () => void;
  onCreated: (user: UserRow) => void;
}

export function CreateUserModal({ open, locales, onClose, onCreated }: CreateUserModalProps) {
  const { t } = useTranslation("admin");
  const formId = useId();
  const [submitting, setSubmitting] = useState(false);
  return (
    <Modal
      open={open}
      title={t("users.create.title")}
      onCancel={() => {
        if (!submitting) {
          onClose();
        }
      }}
      footer={null}
      destroyOnHidden
      centered
      width={560}
      mask={{ closable: false }}
    >
      <CreateUserForm
        formId={formId}
        locales={locales}
        onClose={onClose}
        onCreated={onCreated}
        onBusy={setSubmitting}
      />
    </Modal>
  );
}

interface CreateUserFormProps {
  formId: string;
  locales: readonly string[];
  onClose: () => void;
  onCreated: (user: UserRow) => void;
  onBusy: (busy: boolean) => void;
}

function CreateUserForm({ formId, locales, onClose, onCreated, onBusy }: CreateUserFormProps) {
  const { t } = useTranslation(["admin", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const idempotencyKey = useRef(newIdempotencyKey());
  const [failure, setFailure] = useState<unknown>(null);
  const create = useCreateUser();
  const schema = useMemo(() => createSchema(t as TFunction<"admin">), [t]);
  const {
    control,
    handleSubmit,
    setError,
    setFocus,
    formState: { errors },
  } = useForm<CreateValues>({ resolver: zodResolver(schema), defaultValues: DEFAULTS });
  const hasPassword = useWatch({ control, name: "password" }) !== "";
  const customQuota = useWatch({ control, name: "quotaMode" }) === "custom";
  const options = useMemo(
    () => [
      { value: INSTANCE_LOCALE, label: t("users.create.instanceLocale") },
      ...localeOptions(locales),
    ],
    [locales, t],
  );

  function applyError(error: unknown) {
    if (!(error instanceof ApiError)) {
      setFailure(error);
      return;
    }
    switch (error.code) {
      case "AUTH_RECENT_AUTH_REQUIRED":
        return;
      case "USER_EMAIL_TAKEN":
        setError("email", {
          type: "server",
          message: t("message.emailTaken", { ns: "errors" }),
        });
        setFocus("email");
        return;
      case "USER_USERNAME_TAKEN":
        setError("username", {
          type: "server",
          message: t("message.usernameTaken", { ns: "errors" }),
        });
        setFocus("username");
        return;
      case "PASSWORD_POLICY_VIOLATION": {
        const minLength = detailNumber(error, "minLength");
        setError("password", {
          type: "server",
          message:
            minLength === null
              ? t("message.passwordPolicy", { ns: "errors" })
              : t("validation.passwordTooShort", { minLength }),
        });
        setFocus("password");
        return;
      }
      default: {
        const fields = invalidFields(error, FIELDS);
        if (fields.length === 0) {
          setFailure(reportable(error));
          return;
        }
        for (const field of fields) {
          setError(field, { type: "server", message: t("validation.invalid") });
        }
        setFocus(fields[0] ?? "firstName");
      }
    }
  }

  function submit(values: CreateValues) {
    if (create.isPending) {
      return;
    }
    setFailure(null);
    onBusy(true);
    create.mutate(
      { body: requestOf(values), idempotencyKey: idempotencyKey.current },
      {
        onSuccess: (user) => {
          onBusy(false);
          onCreated(user);
          onClose();
        },
        onError: (error) => {
          onBusy(false);
          applyError(error);
        },
      },
    );
  }

  const pending = create.isPending;

  return (
    <form
      id={formId}
      noValidate
      aria-busy={pending}
      onSubmit={(event) => {
        void handleSubmit(submit)(event);
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false} disabled={pending}>
        <Flex vertical gap={token.marginSM}>
          {failure === null ? null : <ErrorAlert error={failure} />}
          <Flex gap={token.marginSM} wrap>
            <div style={{ flex: "1 1 200px" }}>
              <FormField
                id={id("firstName")}
                label={t("users.create.firstName")}
                error={errors.firstName?.message}
              >
                {(control_) => (
                  <Controller
                    name="firstName"
                    control={control}
                    render={({ field }) => <Input {...field} {...control_} autoComplete="off" />}
                  />
                )}
              </FormField>
            </div>
            <div style={{ flex: "1 1 200px" }}>
              <FormField
                id={id("lastName")}
                label={t("users.create.lastName")}
                error={errors.lastName?.message}
              >
                {(control_) => (
                  <Controller
                    name="lastName"
                    control={control}
                    render={({ field }) => <Input {...field} {...control_} autoComplete="off" />}
                  />
                )}
              </FormField>
            </div>
          </Flex>
          <FormField
            id={id("username")}
            label={t("users.create.username")}
            error={errors.username?.message}
          >
            {(control_) => (
              <Controller
                name="username"
                control={control}
                render={({ field }) => (
                  <Input {...field} {...control_} autoComplete="off" autoCapitalize="none" />
                )}
              />
            )}
          </FormField>
          <FormField id={id("email")} label={t("users.create.email")} error={errors.email?.message}>
            {(control_) => (
              <Controller
                name="email"
                control={control}
                render={({ field }) => (
                  <Input {...field} {...control_} type="email" autoComplete="off" />
                )}
              />
            )}
          </FormField>
          <Flex gap={token.marginSM} wrap>
            <div style={{ flex: "1 1 200px" }}>
              <FormField id={id("role")} label={t("users.create.role")}>
                {(control_) => (
                  <Controller
                    name="role"
                    control={control}
                    render={({ field }) => (
                      <Select<UserRole>
                        {...control_}
                        value={field.value}
                        onChange={field.onChange}
                        options={USER_ROLES.map((role) => ({
                          value: role,
                          label: t(`roles.${role}`),
                        }))}
                      />
                    )}
                  />
                )}
              </FormField>
            </div>
            <div style={{ flex: "1 1 200px" }}>
              <FormField id={id("locale")} label={t("users.create.locale")}>
                {(control_) => (
                  <Controller
                    name="locale"
                    control={control}
                    render={({ field }) => (
                      <Select<string>
                        {...control_}
                        showSearch={{ optionFilterProp: "label" }}
                        value={field.value}
                        onChange={field.onChange}
                        options={options}
                      />
                    )}
                  />
                )}
              </FormField>
            </div>
          </Flex>
          <FormField
            id={id("password")}
            label={t("users.create.password")}
            error={errors.password?.message}
            extra={t(
              hasPassword ? "users.create.passwordHintLocal" : "users.create.passwordHintSso",
            )}
          >
            {(control_) => (
              <Controller
                name="password"
                control={control}
                render={({ field }) => (
                  <Input.Password {...field} {...control_} autoComplete="new-password" />
                )}
              />
            )}
          </FormField>
          <Controller
            name="requirePasswordChange"
            control={control}
            render={({ field }) => (
              <Flex align="center" gap={token.marginXS}>
                <Switch
                  id={id("requirePasswordChange")}
                  checked={hasPassword && field.value}
                  disabled={!hasPassword || pending}
                  onChange={field.onChange}
                />
                <label htmlFor={id("requirePasswordChange")}>
                  {t("users.create.requirePasswordChange")}
                </label>
              </Flex>
            )}
          />
          <Controller
            name="isActive"
            control={control}
            render={({ field }) => (
              <Flex align="center" gap={token.marginXS}>
                <Switch
                  id={id("isActive")}
                  checked={field.value}
                  disabled={pending}
                  onChange={field.onChange}
                />
                <label htmlFor={id("isActive")}>{t("users.create.active")}</label>
              </Flex>
            )}
          />
          <Flex vertical gap={token.marginXS}>
            <Typography.Text strong id={id("quotaLabel")}>
              {t("users.create.quota")}
            </Typography.Text>
            <Controller
              name="quotaMode"
              control={control}
              render={({ field }) => (
                <Radio.Group
                  aria-labelledby={id("quotaLabel")}
                  value={field.value}
                  onChange={field.onChange}
                  options={[
                    { value: "inherit", label: t("users.create.quotaInherit") },
                    { value: "custom", label: t("users.create.quotaCustom") },
                  ]}
                />
              )}
            />
            {customQuota ? (
              <FormField
                id={id("quota")}
                label={t("users.create.quotaAmount")}
                error={errors.quota?.message}
                style={{ marginBottom: 0 }}
              >
                {(control_) => (
                  <Controller
                    name="quota"
                    control={control}
                    render={({ field }) => (
                      <BytesInput {...control_} value={field.value} onChange={field.onChange} />
                    )}
                  />
                )}
              </FormField>
            ) : null}
          </Flex>
        </Flex>
        <Flex justify="end" gap={token.marginXS} style={{ marginTop: token.marginLG }}>
          <Button onClick={onClose} disabled={pending}>
            {t("common.cancel")}
          </Button>
          <Button type="primary" htmlType="submit" loading={pending}>
            {t("users.create.submit")}
          </Button>
        </Flex>
      </Form>
    </form>
  );
}
