import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, InputNumber, Modal, Select, Switch, theme } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ApiError, ErrorAlert } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useCreateInvite } from "../api/mutations";
import { USER_ROLES, type CreatedInvite, type UserRole } from "../types";
import { invalidFields, newIdempotencyKey, reportable } from "./feedback";

const FIELDS = ["email", "role", "expiresInHours"] as const;

interface InviteValues {
  email: string;
  role: UserRole;
  expiresInHours: number | null;
  sendEmail: boolean;
}

function inviteSchema(t: TFunction<"admin">) {
  return z.object({
    email: z
      .string()
      .trim()
      .min(1, t("validation.required"))
      .pipe(z.email(t("validation.email"))),
    role: z.enum(USER_ROLES),
    expiresInHours: z.number().int(t("validation.integer")).nullable(),
    sendEmail: z.boolean(),
  });
}

interface CreateInviteModalProps {
  open: boolean;
  emailEnabled: boolean;
  onClose: () => void;
  onCreated: (invite: CreatedInvite) => void;
}

export function CreateInviteModal({
  open,
  emailEnabled,
  onClose,
  onCreated,
}: CreateInviteModalProps) {
  const { t } = useTranslation("admin");
  const [busy, setBusy] = useState(false);
  return (
    <Modal
      open={open}
      title={t("invites.create.title")}
      onCancel={() => {
        if (!busy) {
          onClose();
        }
      }}
      footer={null}
      destroyOnHidden
      centered
      width={480}
      mask={{ closable: false }}
    >
      <CreateInviteForm
        emailEnabled={emailEnabled}
        onClose={onClose}
        onCreated={onCreated}
        onBusy={setBusy}
      />
    </Modal>
  );
}

interface CreateInviteFormProps {
  emailEnabled: boolean;
  onClose: () => void;
  onCreated: (invite: CreatedInvite) => void;
  onBusy: (busy: boolean) => void;
}

function CreateInviteForm({ emailEnabled, onClose, onCreated, onBusy }: CreateInviteFormProps) {
  const { t } = useTranslation(["admin", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const idempotencyKey = useRef(newIdempotencyKey());
  const [failure, setFailure] = useState<unknown>(null);
  const create = useCreateInvite(onCreated);
  const schema = useMemo(() => inviteSchema(t as TFunction<"admin">), [t]);
  const {
    control,
    handleSubmit,
    setError,
    setFocus,
    formState: { errors },
  } = useForm<InviteValues>({
    resolver: zodResolver(schema),
    defaultValues: { email: "", role: "user", expiresInHours: null, sendEmail: emailEnabled },
  });

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
      default: {
        const fields = invalidFields(error, FIELDS);
        if (fields.length === 0) {
          setFailure(reportable(error));
          return;
        }
        for (const field of fields) {
          setError(field, { type: "server", message: t("validation.invalid") });
        }
        setFocus(fields[0] ?? "email");
      }
    }
  }

  function submit(values: InviteValues) {
    if (create.isPending) {
      return;
    }
    setFailure(null);
    onBusy(true);
    create.mutate(
      {
        body: {
          email: values.email.trim(),
          role: values.role,
          sendEmail: values.sendEmail,
          ...(values.expiresInHours === null ? {} : { expiresInHours: values.expiresInHours }),
        },
        idempotencyKey: idempotencyKey.current,
      },
      {
        onSuccess: () => {
          onBusy(false);
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
      noValidate
      aria-busy={pending}
      onSubmit={(event) => {
        void handleSubmit(submit)(event);
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false} disabled={pending}>
        <Flex vertical gap={token.marginXS}>
          {failure === null ? null : <ErrorAlert error={failure} />}
          <FormField
            id={id("email")}
            label={t("invites.create.email")}
            error={errors.email?.message}
          >
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
          <FormField id={id("role")} label={t("invites.create.role")}>
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
          <FormField
            id={id("expiresInHours")}
            label={t("invites.create.expiry")}
            error={errors.expiresInHours?.message}
            extra={t("invites.create.expiryHint")}
          >
            {(control_) => (
              <Controller
                name="expiresInHours"
                control={control}
                render={({ field }) => (
                  <InputNumber
                    {...control_}
                    min={1}
                    precision={0}
                    value={field.value}
                    onChange={field.onChange}
                    suffix={t("units.hours")}
                    style={{ width: "100%" }}
                  />
                )}
              />
            )}
          </FormField>
          <Controller
            name="sendEmail"
            control={control}
            render={({ field }) => (
              <Flex vertical gap={2}>
                <Flex align="center" gap={token.marginXS}>
                  <Switch
                    id={id("sendEmail")}
                    checked={field.value}
                    disabled={pending}
                    onChange={field.onChange}
                  />
                  <label htmlFor={id("sendEmail")}>{t("invites.create.sendEmail")}</label>
                </Flex>
                <span style={{ color: token.colorTextSecondary, fontSize: token.fontSizeSM }}>
                  {t(emailEnabled ? "invites.create.sendEmailHint" : "invites.create.sendEmailOff")}
                </span>
              </Flex>
            )}
          />
        </Flex>
        <Flex justify="end" gap={token.marginXS} style={{ marginTop: token.marginLG }}>
          <Button onClick={onClose} disabled={pending}>
            {t("common.cancel")}
          </Button>
          <Button type="primary" htmlType="submit" loading={pending}>
            {t("invites.create.submit")}
          </Button>
        </Flex>
      </Form>
    </form>
  );
}
