import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, InputNumber, Switch, theme } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { FormField } from "../../../shared/ui/FormField";
import { useUpdatePublicLinkSettings } from "../api/mutations";
import type { PublicLinkPatch, PublicLinkSettings } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { mapSettingsError } from "./settingsErrors";

interface PublicLinkValues {
  unlimited: boolean;
  days: number | null;
}

function valuesOf(settings: PublicLinkSettings): PublicLinkValues {
  return {
    unlimited: settings.maxPublicLinkLifetimeDays === null,
    days: settings.maxPublicLinkLifetimeDays,
  };
}

export function publicLinkPatchOf(
  current: PublicLinkSettings,
  values: PublicLinkValues,
): PublicLinkPatch {
  const next = values.unlimited ? null : values.days;
  return next === current.maxPublicLinkLifetimeDays ? {} : { maxPublicLinkLifetimeDays: next };
}

function publicLinkSchema(t: TFunction<"admin">) {
  return z
    .object({ unlimited: z.boolean(), days: z.number().nullable() })
    .superRefine((values, context) => {
      if (values.unlimited) {
        return;
      }
      if (values.days === null) {
        context.addIssue({ code: "custom", path: ["days"], message: t("validation.required") });
      } else if (!Number.isInteger(values.days)) {
        context.addIssue({ code: "custom", path: ["days"], message: t("validation.integer") });
      }
    });
}

export function PublicLinkSettingsForm({ settings }: { settings: PublicLinkSettings }) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const feedback = useActionFeedback<"saved">();
  const update = useUpdatePublicLinkSettings();
  const schema = useMemo(() => publicLinkSchema(t), [t]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors, isDirty },
  } = useForm<PublicLinkValues>({
    resolver: zodResolver(schema),
    defaultValues: valuesOf(settings),
  });
  const unlimited = useWatch({ control, name: "unlimited" });

  function submit(values: PublicLinkValues) {
    if (update.isPending) {
      return;
    }
    const patch = publicLinkPatchOf(settings, values);
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
        const failure = mapSettingsError(error, t, ["maxPublicLinkLifetimeDays"]);
        if (failure === null) {
          return;
        }
        if (failure.kind === "general") {
          feedback.fail(failure.error);
          return;
        }
        setError("days", { type: "server", message: failure.message });
        setFocus("days");
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
        <Flex vertical gap={token.marginXS} style={{ maxWidth: 420 }}>
          <FeedbackAlerts feedback={feedback} noticeKey={() => "settings.saved"} />
          <Controller
            name="unlimited"
            control={control}
            render={({ field }) => (
              <Flex align="center" gap={token.marginXS}>
                <Switch
                  id={id("unlimited")}
                  checked={field.value}
                  onChange={field.onChange}
                  disabled={pending}
                />
                <label htmlFor={id("unlimited")}>{t("settings.publicLinks.noMaximum")}</label>
              </Flex>
            )}
          />
          <FormField
            id={id("days")}
            label={t("settings.publicLinks.maxLifetime")}
            error={errors.days?.message}
            extra={t("settings.publicLinks.hint")}
          >
            {(control_) => (
              <Controller
                name="days"
                control={control}
                render={({ field }) => (
                  <InputNumber
                    {...control_}
                    precision={0}
                    disabled={unlimited || pending}
                    value={field.value}
                    onChange={field.onChange}
                    suffix={t("units.days")}
                    style={{ width: "100%" }}
                  />
                )}
              />
            )}
          </FormField>
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
