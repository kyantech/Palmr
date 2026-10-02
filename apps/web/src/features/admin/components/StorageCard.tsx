import { zodResolver } from "@hookform/resolvers/zod";
import { Alert, Button, Flex, Form, Radio, Skeleton, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { formatBytes, splitBytes } from "../../../shared/format/bytes";
import { FormField } from "../../../shared/ui/FormField";
import { useSetUserQuota } from "../api/mutations";
import { useSettings } from "../api/queries";
import type { QuotaMode, QuotaOverrideRequest, UserDetail } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { BytesInput, type BytesValue, bytesOf, isWithinByteRange } from "./BytesInput";
import { FactList } from "./FactList";
import { Section } from "./Section";
import { StorageUsage } from "./StorageUsage";

interface QuotaValues {
  mode: QuotaMode;
  amount: BytesValue;
}

function quotaSchema(t: TFunction<"admin">) {
  return z
    .object({
      mode: z.enum(["inherit", "unlimited", "bytes"]),
      amount: z.object({
        amount: z.number().nullable(),
        unit: z.enum(["B", "KiB", "MiB", "GiB", "TiB", "PiB"]),
      }),
    })
    .superRefine((values, context) => {
      if (values.mode === "bytes" && !isWithinByteRange(values.amount)) {
        context.addIssue({ code: "custom", path: ["amount"], message: t("validation.quota") });
      }
    });
}

function valuesOf(mode: QuotaMode, quotaBytes: number | null): QuotaValues {
  return {
    mode,
    amount: quotaBytes === null ? { amount: null, unit: "GiB" } : splitBytes(quotaBytes),
  };
}

function requestOf(values: QuotaValues): QuotaOverrideRequest {
  if (values.mode === "bytes") {
    return { mode: "bytes", quotaBytes: bytesOf(values.amount) };
  }
  return { mode: values.mode };
}

function quotaModeOf(mode: string): QuotaMode {
  return mode === "bytes" || mode === "unlimited" ? mode : "inherit";
}

export function StorageCard({ user }: { user: UserDetail }) {
  const { t } = useTranslation("admin");
  const quotas = useSettings("quotas");
  if (quotas.isPending) {
    return (
      <Section
        title={t("detail.storage.title")}
        description={t("detail.storage.description")}
        testId="user-storage"
      >
        <Skeleton active paragraph={{ rows: 4 }} />
      </Section>
    );
  }
  return <StorageCardBody user={user} instanceDefault={quotas.data?.defaultUserQuotaBytes} />;
}

interface StorageCardBodyProps {
  user: UserDetail;
  instanceDefault: number | null | undefined;
}

function StorageCardBody({ user, instanceDefault }: StorageCardBodyProps) {
  const { t, i18n } = useTranslation("admin");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const feedback = useActionFeedback<"saved">();
  const setQuota = useSetUserQuota();
  const schema = useMemo(() => quotaSchema(t), [t]);
  const {
    control,
    handleSubmit,
    reset,
    formState: { errors, isDirty },
  } = useForm<QuotaValues>({
    resolver: zodResolver(schema),
    defaultValues: valuesOf(user.quotaOverrideMode, user.quotaBytes),
  });
  const mode = useWatch({ control, name: "mode" });
  const locale = i18n.language;
  const describe = (bytes: number | null | undefined) =>
    bytes === undefined
      ? t("common.unknown")
      : bytes === null
        ? t("quota.unlimited")
        : formatBytes(bytes, locale);

  function submit(values: QuotaValues) {
    if (setQuota.isPending) {
      return;
    }
    feedback.clear();
    setQuota.mutate(
      { userId: user.id, body: requestOf(values) },
      {
        onSuccess: (policy) => {
          reset(valuesOf(quotaModeOf(policy.mode), policy.quotaBytes));
          feedback.succeed("saved");
        },
        onError: feedback.fail,
      },
    );
  }

  const pending = setQuota.isPending;
  const policy = setQuota.data;

  return (
    <Section
      title={t("detail.storage.title")}
      description={t("detail.storage.description")}
      testId="user-storage"
    >
      <Flex vertical gap={token.margin}>
        <StorageUsage
          usedBytes={user.usedBytes}
          effectiveQuotaBytes={user.effectiveQuotaBytes}
          overQuota={user.overQuota}
          width={260}
        />
        <FactList
          facts={[
            {
              key: "used",
              label: t("detail.storage.used"),
              value: formatBytes(user.usedBytes, locale),
            },
            {
              key: "effective",
              label: t("detail.storage.effective"),
              value: describe(user.effectiveQuotaBytes),
            },
            {
              key: "default",
              label: t("detail.storage.instanceDefault"),
              value: describe(instanceDefault),
            },
            {
              key: "files",
              label: t("detail.storage.files"),
              value: String(user.counts.files),
            },
            {
              key: "received",
              label: t("detail.storage.receivedFiles"),
              value: String(user.counts.receivedFiles),
            },
            { key: "shares", label: t("detail.storage.shares"), value: String(user.counts.shares) },
            {
              key: "reverseShares",
              label: t("detail.storage.reverseShares"),
              value: String(user.counts.reverseShares),
            },
          ]}
        />
        <form
          noValidate
          aria-busy={pending}
          aria-label={t("detail.storage.overrideTitle")}
          onSubmit={(event) => {
            void handleSubmit(submit)(event);
          }}
        >
          <Form layout="vertical" component={false} requiredMark={false} disabled={pending}>
            <Flex vertical gap={token.marginXS}>
              <Typography.Text strong id={`${idPrefix}-override`}>
                {t("detail.storage.overrideTitle")}
              </Typography.Text>
              <FeedbackAlerts feedback={feedback} noticeKey={() => "detail.storage.saved"} />
              {policy?.belowCurrentUsage === true && feedback.notice === "saved" ? (
                <Alert
                  type="warning"
                  showIcon
                  role="status"
                  data-testid="below-usage-warning"
                  title={t("detail.storage.belowUsage.title")}
                  description={t("detail.storage.belowUsage.description")}
                />
              ) : null}
              <Controller
                name="mode"
                control={control}
                render={({ field }) => (
                  <Radio.Group
                    aria-labelledby={`${idPrefix}-override`}
                    value={field.value}
                    onChange={field.onChange}
                    style={{ display: "flex", flexDirection: "column", gap: token.marginXXS }}
                    options={[
                      {
                        value: "inherit",
                        label: t("detail.storage.modes.inherit", {
                          value: describe(instanceDefault),
                        }),
                      },
                      { value: "unlimited", label: t("detail.storage.modes.unlimited") },
                      { value: "bytes", label: t("detail.storage.modes.bytes") },
                    ]}
                  />
                )}
              />
              {mode === "bytes" ? (
                <FormField
                  id={`${idPrefix}-amount`}
                  label={t("detail.storage.amount")}
                  error={errors.amount?.message}
                  style={{ marginBottom: 0, maxWidth: 320 }}
                >
                  {(control_) => (
                    <Controller
                      name="amount"
                      control={control}
                      render={({ field }) => (
                        <BytesInput {...control_} value={field.value} onChange={field.onChange} />
                      )}
                    />
                  )}
                </FormField>
              ) : null}
              <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
                {t("detail.storage.noBypass")}
              </Typography.Text>
              <div>
                <Button type="primary" htmlType="submit" loading={pending} disabled={!isDirty}>
                  {t("detail.storage.save")}
                </Button>
              </div>
            </Flex>
          </Form>
        </form>
      </Flex>
    </Section>
  );
}
