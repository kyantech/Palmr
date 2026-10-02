import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, theme } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { splitBytes } from "../../../shared/format/bytes";
import { FormField } from "../../../shared/ui/FormField";
import { useUpdateQuotaSettings } from "../api/mutations";
import type { QuotaPatch, QuotaSettings } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import {
  isWithinByteRange,
  OptionalBytesInput,
  type OptionalBytesValue,
  optionalBytesOf,
} from "./BytesInput";
import { mapSettingsError } from "./settingsErrors";

type QuotaKey = "defaultUserQuotaBytes" | "maxFileSizeBytes";

const QUOTA_KEYS: readonly QuotaKey[] = ["defaultUserQuotaBytes", "maxFileSizeBytes"];

type QuotaValues = Record<QuotaKey, OptionalBytesValue>;

function fieldOf(bytes: number | null): OptionalBytesValue {
  return bytes === null
    ? { unlimited: true, amount: { amount: null, unit: "GiB" } }
    : { unlimited: false, amount: splitBytes(bytes) };
}

function valuesOf(settings: QuotaSettings): QuotaValues {
  return {
    defaultUserQuotaBytes: fieldOf(settings.defaultUserQuotaBytes),
    maxFileSizeBytes: fieldOf(settings.maxFileSizeBytes),
  };
}

export function quotaPatchOf(current: QuotaSettings, values: QuotaValues): QuotaPatch {
  const patch: QuotaPatch = {};
  for (const key of QUOTA_KEYS) {
    const next = optionalBytesOf(values[key]);
    if (next !== current[key]) {
      patch[key] = next;
    }
  }
  return patch;
}

function quotaSchema(t: TFunction<"admin">) {
  const field = () =>
    z
      .object({
        unlimited: z.boolean(),
        amount: z.object({
          amount: z.number().nullable(),
          unit: z.enum(["B", "KiB", "MiB", "GiB", "TiB", "PiB"]),
        }),
      })
      .refine((value) => value.unlimited || isWithinByteRange(value.amount), {
        message: t("validation.quota"),
      });
  return z.object({ defaultUserQuotaBytes: field(), maxFileSizeBytes: field() });
}

export function QuotaSettingsForm({ settings }: { settings: QuotaSettings }) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const feedback = useActionFeedback<"saved">();
  const update = useUpdateQuotaSettings();
  const schema = useMemo(() => quotaSchema(t), [t]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors, isDirty },
  } = useForm<QuotaValues>({ resolver: zodResolver(schema), defaultValues: valuesOf(settings) });

  function submit(values: QuotaValues) {
    if (update.isPending) {
      return;
    }
    const patch = quotaPatchOf(settings, values);
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
        const failure = mapSettingsError(error, t, QUOTA_KEYS);
        if (failure === null) {
          return;
        }
        if (failure.kind === "general") {
          feedback.fail(failure.error);
          return;
        }
        const field = failure.key as QuotaKey;
        setError(field, { type: "server", message: failure.message });
        setFocus(`${field}.amount.amount`);
      },
    });
  }

  const pending = update.isPending;

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
          {QUOTA_KEYS.map((key) => (
            <FormField
              key={key}
              id={id(key)}
              label={t(`settings.quotas.fields.${key}`)}
              error={errors[key]?.message ?? errors[key]?.root?.message}
              extra={t(`settings.quotas.hints.${key}`)}
              style={{ maxWidth: 420 }}
            >
              {(control_) => (
                <Controller
                  name={key}
                  control={control}
                  render={({ field }) => (
                    <OptionalBytesInput
                      {...control_}
                      value={field.value}
                      onChange={field.onChange}
                      unlimitedLabel={t("quota.unlimited")}
                    />
                  )}
                />
              )}
            </FormField>
          ))}
          <div>
            <Button type="primary" htmlType="submit" loading={pending} disabled={!isDirty}>
              {t("settings.save")}
            </Button>
          </div>
        </Flex>
      </Form>
    </form>
  );
}
