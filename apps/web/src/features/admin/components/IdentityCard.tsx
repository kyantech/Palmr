import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, theme } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ApiError } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { FormField } from "../../../shared/ui/FormField";
import { useUpdateUser } from "../api/mutations";
import type { UpdateUserRequest, UserDetail } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { invalidFields } from "./feedback";
import { Section } from "./Section";

const FIELDS = ["firstName", "lastName", "username"] as const;

interface IdentityValues {
  firstName: string;
  lastName: string;
  username: string;
}

function identitySchema(t: TFunction<"admin">) {
  const required = t("validation.required");
  return z.object({
    firstName: z.string().trim().min(1, required),
    lastName: z.string().trim().min(1, required),
    username: z.string().trim().min(1, required),
  });
}

function valuesOf(user: Pick<UserDetail, "firstName" | "lastName" | "username">): IdentityValues {
  return { firstName: user.firstName, lastName: user.lastName, username: user.username };
}

export function IdentityCard({ user }: { user: UserDetail }) {
  const { t, i18n } = useTranslation(["admin", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const feedback = useActionFeedback<"saved">();
  const update = useUpdateUser();
  const schema = useMemo(() => identitySchema(t as TFunction<"admin">), [t]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors, dirtyFields, isDirty },
  } = useForm<IdentityValues>({ resolver: zodResolver(schema), defaultValues: valuesOf(user) });

  function applyError(error: unknown) {
    if (!(error instanceof ApiError)) {
      feedback.fail(error);
      return;
    }
    if (error.code === "AUTH_RECENT_AUTH_REQUIRED") {
      return;
    }
    if (error.code === "USER_USERNAME_TAKEN") {
      setError("username", {
        type: "server",
        message: t("message.usernameTaken", { ns: "errors" }),
      });
      setFocus("username");
      return;
    }
    const fields = invalidFields(error, FIELDS);
    if (fields.length === 0) {
      feedback.fail(error);
      return;
    }
    for (const field of fields) {
      setError(field, { type: "server", message: t("validation.invalid") });
    }
    setFocus(fields[0] ?? "firstName");
  }

  function submit(values: IdentityValues) {
    if (update.isPending) {
      return;
    }
    feedback.clear();
    const body: UpdateUserRequest = {
      ...(dirtyFields.firstName === true ? { firstName: values.firstName.trim() } : {}),
      ...(dirtyFields.lastName === true ? { lastName: values.lastName.trim() } : {}),
      ...(dirtyFields.username === true ? { username: values.username.trim() } : {}),
    };
    update.mutate(
      { userId: user.id, body },
      {
        onSuccess: (saved) => {
          reset(valuesOf(saved));
          feedback.succeed("saved");
        },
        onError: applyError,
      },
    );
  }

  const pending = update.isPending;

  return (
    <Section
      title={t("detail.identity.title")}
      description={t("detail.identity.description")}
      testId="user-identity"
    >
      <form
        noValidate
        aria-busy={pending}
        onSubmit={(event) => {
          void handleSubmit(submit)(event);
        }}
      >
        <Form layout="vertical" component={false} requiredMark={false} disabled={pending}>
          <Flex vertical gap={token.marginXS}>
            <FeedbackAlerts feedback={feedback} noticeKey={() => "detail.identity.saved"} />
            <Flex gap={token.marginSM} wrap>
              <div style={{ flex: "1 1 200px" }}>
                <FormField
                  id={id("firstName")}
                  label={t("detail.identity.firstName")}
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
                  label={t("detail.identity.lastName")}
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
              label={t("detail.identity.username")}
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
            <Flex justify="space-between" align="center" gap={token.marginSM} wrap>
              <span style={{ color: token.colorTextSecondary, fontSize: token.fontSizeSM }}>
                {t("detail.identity.memberSince", {
                  date: formatDateTime(user.createdAt, i18n.language),
                })}
              </span>
              <Button type="primary" htmlType="submit" loading={pending} disabled={!isDirty}>
                {t("detail.identity.save")}
              </Button>
            </Flex>
          </Flex>
        </Form>
      </form>
    </Section>
  );
}
