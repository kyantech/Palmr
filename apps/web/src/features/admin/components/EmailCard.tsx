import { zodResolver } from "@hookform/resolvers/zod";
import { Alert, Button, Flex, Form, Input, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ApiError } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useCancelEmailChange, useResendEmailChange, useStartEmailChange } from "../api/mutations";
import type { UserDetail } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { ConfirmDialog } from "./ConfirmDialog";
import { Section } from "./Section";

interface EmailValues {
  email: string;
}

type EmailNotice = "requested" | "resent" | "cancelled";

function emailSchema(t: TFunction<"admin">) {
  return z.object({
    email: z
      .string()
      .trim()
      .min(1, t("validation.required"))
      .pipe(z.email(t("validation.email"))),
  });
}

export function EmailCard({ user }: { user: UserDetail }) {
  const { t } = useTranslation(["admin", "errors"]);
  const { token } = theme.useToken();
  const fieldId = useId();
  const feedback = useActionFeedback<EmailNotice>();
  const start = useStartEmailChange();
  const resend = useResendEmailChange();
  const cancel = useCancelEmailChange();
  const [cancelling, setCancelling] = useState(false);
  const schema = useMemo(() => emailSchema(t as TFunction<"admin">), [t]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors },
  } = useForm<EmailValues>({ resolver: zodResolver(schema), defaultValues: { email: "" } });

  function submit({ email }: EmailValues) {
    if (start.isPending) {
      return;
    }
    feedback.clear();
    start.mutate(
      { userId: user.id, email: email.trim() },
      {
        onSuccess: () => {
          reset({ email: "" });
          feedback.succeed("requested");
        },
        onError: (error) => {
          if (error instanceof ApiError && error.code === "USER_EMAIL_TAKEN") {
            setError("email", {
              type: "server",
              message: t("message.emailTaken", { ns: "errors" }),
            });
            setFocus("email");
            return;
          }
          feedback.fail(error);
        },
      },
    );
  }

  const busy = start.isPending || resend.isPending || cancel.isPending;

  return (
    <Section
      title={t("detail.email.title")}
      description={t("detail.email.description")}
      testId="user-email"
    >
      <Flex vertical gap={token.margin}>
        <Flex vertical gap={2}>
          <Typography.Text type="secondary">{t("detail.email.current")}</Typography.Text>
          <Typography.Text strong data-testid="canonical-email">
            {user.email}
          </Typography.Text>
        </Flex>
        <FeedbackAlerts
          feedback={feedback}
          noticeKey={(notice) => `detail.email.notice.${notice}`}
        />
        {user.pendingEmail === null ? null : (
          <Alert
            type="warning"
            showIcon
            data-testid="pending-email-notice"
            title={t("detail.email.pendingTitle", { email: user.pendingEmail })}
            description={t("detail.email.pendingDescription", { email: user.email })}
            action={
              <Flex gap={token.marginXS} wrap>
                <Button
                  size="small"
                  loading={resend.isPending}
                  disabled={busy}
                  onClick={() => {
                    feedback.clear();
                    resend.mutate(user.id, {
                      onSuccess: () => {
                        feedback.succeed("resent");
                      },
                      onError: feedback.fail,
                    });
                  }}
                >
                  {t("detail.email.resend")}
                </Button>
                <Button
                  size="small"
                  danger
                  disabled={busy}
                  onClick={() => {
                    setCancelling(true);
                  }}
                >
                  {t("detail.email.cancel")}
                </Button>
              </Flex>
            }
          />
        )}
        <form
          noValidate
          aria-busy={start.isPending}
          onSubmit={(event) => {
            void handleSubmit(submit)(event);
          }}
        >
          <Form layout="vertical" component={false} requiredMark={false} disabled={busy}>
            <FormField
              id={fieldId}
              label={t("detail.email.newLabel")}
              error={errors.email?.message}
              extra={t("detail.email.hint")}
              style={{ marginBottom: token.marginSM }}
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
            <Button htmlType="submit" loading={start.isPending}>
              {t("detail.email.request")}
            </Button>
          </Form>
        </form>
      </Flex>
      <ConfirmDialog
        open={cancelling}
        title={t("detail.email.cancelDialog.title")}
        description={
          <Typography.Text>
            {t("detail.email.cancelDialog.description", { email: user.pendingEmail ?? "" })}
          </Typography.Text>
        }
        confirmLabel={t("detail.email.cancelDialog.confirm")}
        danger
        loading={cancel.isPending}
        onCancel={() => {
          setCancelling(false);
        }}
        onConfirm={() => {
          feedback.clear();
          cancel.mutate(user.id, {
            onSuccess: () => {
              setCancelling(false);
              feedback.succeed("cancelled");
            },
            onError: (error) => {
              setCancelling(false);
              feedback.fail(error);
            },
          });
        }}
      />
    </Section>
  );
}
