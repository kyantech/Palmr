import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, Select, Switch, theme } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { localeOptions } from "../../../shared/format/locale";
import { FormField } from "../../../shared/ui/FormField";
import { useUpdateGeneralSettings } from "../api/mutations";
import {
  type GeneralPatch,
  type GeneralSettings,
  THUMBNAIL_LIMITS,
  type ThumbnailSourceLimit,
} from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { changedKeys, mapSettingsError } from "./settingsErrors";

interface GeneralValues {
  appName: string;
  appDescription: string;
  defaultLocale: string;
  showVersion: boolean;
  poweredByVisible: boolean;
  thumbnailSourceLimit: ThumbnailSourceLimit;
}

const FIELD_OF_KEY: Readonly<Record<string, keyof GeneralValues>> = {
  appName: "appName",
  appDescription: "appDescription",
  defaultLocale: "defaultLocale",
  hideVersion: "showVersion",
  poweredByVisible: "poweredByVisible",
  thumbnailSourceLimit: "thumbnailSourceLimit",
};

function valuesOf(settings: GeneralSettings): GeneralValues {
  return {
    appName: settings.appName,
    appDescription: settings.appDescription,
    defaultLocale: settings.defaultLocale,
    showVersion: !settings.hideVersion,
    poweredByVisible: settings.poweredByVisible,
    thumbnailSourceLimit: settings.thumbnailSourceLimit,
  };
}

function patchOf(current: GeneralSettings, values: GeneralValues): GeneralPatch {
  return changedKeys<GeneralSettings>(current, {
    appName: values.appName.trim(),
    appDescription: values.appDescription.trim(),
    defaultLocale: values.defaultLocale,
    hideVersion: !values.showVersion,
    poweredByVisible: values.poweredByVisible,
    thumbnailSourceLimit: values.thumbnailSourceLimit,
  });
}

function generalSchema(t: TFunction<"admin">) {
  return z.object({
    appName: z.string().trim().min(1, t("validation.required")),
    appDescription: z.string(),
    defaultLocale: z.string().min(1, t("validation.required")),
    showVersion: z.boolean(),
    poweredByVisible: z.boolean(),
    thumbnailSourceLimit: z.enum(THUMBNAIL_LIMITS),
  });
}

interface GeneralSettingsFormProps {
  settings: GeneralSettings;
  locales: readonly string[];
}

export function GeneralSettingsForm({ settings, locales }: GeneralSettingsFormProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: string) => `${idPrefix}-${field}`;
  const feedback = useActionFeedback<"saved">();
  const update = useUpdateGeneralSettings();
  const schema = useMemo(() => generalSchema(t), [t]);
  const options = useMemo(() => localeOptions(locales), [locales]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors, isDirty },
  } = useForm<GeneralValues>({ resolver: zodResolver(schema), defaultValues: valuesOf(settings) });

  function submit(values: GeneralValues) {
    if (update.isPending) {
      return;
    }
    const patch = patchOf(settings, values);
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
        const failure = mapSettingsError(error, t, Object.keys(FIELD_OF_KEY));
        if (failure === null) {
          return;
        }
        if (failure.kind === "general") {
          feedback.fail(failure.error);
          return;
        }
        const field = FIELD_OF_KEY[failure.key];
        if (field !== undefined) {
          setError(field, { type: "server", message: failure.message });
          setFocus(field);
        }
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
          <FormField
            id={id("appName")}
            label={t("settings.general.appName")}
            error={errors.appName?.message}
          >
            {(control_) => (
              <Controller
                name="appName"
                control={control}
                render={({ field }) => <Input {...field} {...control_} />}
              />
            )}
          </FormField>
          <FormField
            id={id("appDescription")}
            label={t("settings.general.appDescription")}
            error={errors.appDescription?.message}
          >
            {(control_) => (
              <Controller
                name="appDescription"
                control={control}
                render={({ field }) => <Input.TextArea {...field} {...control_} rows={2} />}
              />
            )}
          </FormField>
          <Flex gap={token.marginSM} wrap>
            <div style={{ flex: "1 1 240px" }}>
              <FormField
                id={id("defaultLocale")}
                label={t("settings.general.defaultLocale")}
                error={errors.defaultLocale?.message}
              >
                {(control_) => (
                  <Controller
                    name="defaultLocale"
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
            <div style={{ flex: "1 1 240px" }}>
              <FormField
                id={id("thumbnailSourceLimit")}
                label={t("settings.general.thumbnailSourceLimit")}
                error={errors.thumbnailSourceLimit?.message}
                extra={t("settings.general.thumbnailSourceLimitHint")}
              >
                {(control_) => (
                  <Controller
                    name="thumbnailSourceLimit"
                    control={control}
                    render={({ field }) => (
                      <Select<ThumbnailSourceLimit>
                        {...control_}
                        value={field.value}
                        onChange={field.onChange}
                        options={THUMBNAIL_LIMITS.map((limit) => ({
                          value: limit,
                          label: t(`settings.general.thumbnailLimits.${limit}`),
                        }))}
                      />
                    )}
                  />
                )}
              </FormField>
            </div>
          </Flex>
          <Controller
            name="showVersion"
            control={control}
            render={({ field }) => (
              <Flex align="center" gap={token.marginXS} style={{ marginBottom: token.marginXS }}>
                <Switch
                  id={id("showVersion")}
                  checked={field.value}
                  onChange={field.onChange}
                  disabled={pending}
                />
                <label htmlFor={id("showVersion")}>{t("settings.general.showVersion")}</label>
              </Flex>
            )}
          />
          <Controller
            name="poweredByVisible"
            control={control}
            render={({ field }) => (
              <Flex align="center" gap={token.marginXS} style={{ marginBottom: token.marginSM }}>
                <Switch
                  id={id("poweredByVisible")}
                  checked={field.value}
                  onChange={field.onChange}
                  disabled={pending}
                />
                <label htmlFor={id("poweredByVisible")}>
                  {t("settings.general.poweredByVisible")}
                </label>
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
  );
}
