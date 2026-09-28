import { Flex, Radio, Select, theme, Typography } from "antd";
import { type ReactNode, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { localeOptions } from "../../../shared/format/locale";
import { useUpdatePreferences } from "../api/mutations";
import {
  type AccentPreset,
  type PreferenceChange,
  type Preferences,
  THEME_OPTIONS,
  type ThemeOption,
} from "../types";
import { SettingsSection } from "./SettingsSection";

interface AppearanceFormProps {
  preferences: Preferences;
  accents: readonly AccentPreset[];
  locales: readonly string[];
}

function isThemeOption(value: unknown): value is ThemeOption {
  return THEME_OPTIONS.some((option) => option === value);
}

function Field({
  labelId,
  label,
  children,
}: {
  labelId: string;
  label: string;
  children: ReactNode;
}) {
  const { token } = theme.useToken();
  return (
    <Flex vertical gap={token.marginXS}>
      <Typography.Text id={labelId} strong>
        {label}
      </Typography.Text>
      {children}
    </Flex>
  );
}

export function AppearanceForm({ preferences, accents, locales }: AppearanceFormProps) {
  const { t } = useTranslation("settings");
  const { token } = theme.useToken();
  const themeLabelId = useId();
  const accentLabelId = useId();
  const localeLabelId = useId();
  const localeInputId = useId();
  const updatePreferences = useUpdatePreferences();
  const [failure, setFailure] = useState<unknown>(null);
  const [saved, setSaved] = useState(false);
  const languageOptions = useMemo(() => localeOptions(locales), [locales]);

  const save = (change: PreferenceChange) => {
    setFailure(null);
    setSaved(false);
    updatePreferences.mutate(change, {
      onSuccess: () => {
        setSaved(true);
      },
      onError: (error) => {
        setFailure(error);
      },
    });
  };

  const status = updatePreferences.isPending
    ? t("appearance.saving")
    : saved
      ? t("appearance.saved")
      : null;

  return (
    <SettingsSection
      title={t("appearance.title")}
      description={t("appearance.description")}
      extra={
        <Typography.Text
          type={updatePreferences.isPending ? "secondary" : "success"}
          role="status"
          style={{ minHeight: token.lineHeight * token.fontSize }}
        >
          {status}
        </Typography.Text>
      }
      testId="settings-appearance"
    >
      <Flex vertical gap={token.marginXL}>
        {failure === null ? null : <ErrorAlert error={failure} />}
        <Field labelId={themeLabelId} label={t("appearance.theme.label")}>
          <Radio.Group
            aria-labelledby={themeLabelId}
            optionType="button"
            buttonStyle="solid"
            value={isThemeOption(preferences.theme) ? preferences.theme : null}
            onChange={(event) => {
              const value: unknown = event.target.value;
              if (isThemeOption(value)) {
                save({ theme: value });
              }
            }}
            options={THEME_OPTIONS.map((option) => ({
              value: option,
              label: t(`appearance.theme.options.${option}`),
            }))}
          />
        </Field>
        <Field labelId={accentLabelId} label={t("appearance.accent.label")}>
          <Radio.Group
            aria-labelledby={accentLabelId}
            optionType="button"
            value={
              accents.some((accent) => accent.key === preferences.accent)
                ? preferences.accent
                : null
            }
            onChange={(event) => {
              const value: unknown = event.target.value;
              if (typeof value === "string") {
                save({ accent: value });
              }
            }}
            style={{ display: "flex", flexWrap: "wrap", gap: token.marginXS }}
          >
            {accents.map((accent) => (
              <Radio.Button
                key={accent.key}
                value={accent.key}
                data-accent={accent.key}
                style={{
                  borderRadius: token.borderRadius,
                  borderInlineStartWidth: token.lineWidth,
                }}
              >
                <Flex align="center" gap={token.marginXS} style={{ height: "100%" }}>
                  <span
                    aria-hidden="true"
                    style={{
                      display: "inline-block",
                      width: 14,
                      height: 14,
                      borderRadius: "50%",
                      background: accent.color,
                      boxShadow: `0 0 0 1px ${token.colorBorder}`,
                    }}
                  />
                  {t(`appearance.accent.options.${accent.key}`)}
                </Flex>
              </Radio.Button>
            ))}
          </Radio.Group>
        </Field>
        <Field labelId={localeLabelId} label={t("appearance.locale.label")}>
          <Select
            id={localeInputId}
            aria-labelledby={localeLabelId}
            showSearch={{ optionFilterProp: "label" }}
            virtual={false}
            value={preferences.locale}
            options={languageOptions}
            style={{ width: "100%", maxWidth: 360 }}
            onChange={(value: string) => {
              save({ locale: value });
            }}
          />
        </Field>
      </Flex>
    </SettingsSection>
  );
}
