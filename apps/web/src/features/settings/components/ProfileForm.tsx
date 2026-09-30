import { zodResolver } from "@hookform/resolvers/zod";
import { Button, Flex, Form, Input, theme, Typography } from "antd";
import type { TFunction } from "i18next";
import { useId, useMemo, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ErrorAlert } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useUpdateProfile } from "../api/mutations";
import type { Profile, ProfileChange } from "../types";
import { characters, invalidFields } from "./formErrors";
import { SettingsSection } from "../../../shared/ui/SettingsSection";

const DISPLAY_TEXT_MAX = 100;
const CONTROL_CHARACTER = /\p{Cc}/u;

const PROFILE_FIELDS = [
  "firstName",
  "lastName",
] as const satisfies readonly (keyof ProfileChange)[];

function profileSchema(t: TFunction<"settings">) {
  const name = z
    .string()
    .trim()
    .min(1, t("profile.validation.required"))
    .refine((value) => characters(value) <= DISPLAY_TEXT_MAX, {
      message: t("profile.validation.tooLong", { max: DISPLAY_TEXT_MAX }),
    })
    .refine((value) => !CONTROL_CHARACTER.test(value), {
      message: t("profile.validation.invalid"),
    });
  return z.object({ firstName: name, lastName: name });
}

interface ProfileFormProps {
  profile: Profile;
}

export function ProfileForm({ profile }: ProfileFormProps) {
  const { t } = useTranslation("settings");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const id = (field: keyof ProfileChange) => `${idPrefix}-${field}`;
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const [saved, setSaved] = useState(false);
  const updateProfile = useUpdateProfile();
  const schema = useMemo(() => profileSchema(t), [t]);
  const {
    control,
    handleSubmit,
    reset,
    setError,
    setFocus,
    formState: { errors, isDirty, isSubmitting },
  } = useForm<ProfileChange>({
    resolver: zodResolver(schema),
    defaultValues: { firstName: profile.firstName, lastName: profile.lastName },
  });

  async function submit(values: ProfileChange) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    setSaved(false);
    try {
      const updated = await updateProfile.mutateAsync(values);
      reset({ firstName: updated.firstName, lastName: updated.lastName });
      setSaved(true);
    } catch (error) {
      const fields = invalidFields(error, PROFILE_FIELDS);
      if (fields.length === 0) {
        setFailure(error);
        return;
      }
      for (const field of fields) {
        setError(field, { type: "server", message: t("profile.validation.invalid") });
      }
      setFocus(fields[0] ?? "firstName");
    } finally {
      inFlight.current = false;
    }
  }

  return (
    <SettingsSection
      title={t("profile.title")}
      description={t("profile.description")}
      testId="settings-profile"
    >
      <form
        noValidate
        aria-busy={isSubmitting}
        onSubmit={(event) => {
          void handleSubmit(submit)(event);
        }}
      >
        <Form layout="vertical" component={false} requiredMark={false} disabled={isSubmitting}>
          {failure === null ? null : (
            <div style={{ marginBottom: token.marginLG }}>
              <ErrorAlert error={failure} />
            </div>
          )}
          <Flex gap={token.margin} wrap>
            {PROFILE_FIELDS.map((field) => (
              <div key={field} style={{ flex: "1 1 240px", minWidth: 0 }}>
                <FormField
                  id={id(field)}
                  label={t(`profile.${field}`)}
                  error={errors[field]?.message}
                >
                  {(fieldProps) => (
                    <Controller
                      name={field}
                      control={control}
                      render={({ field: input }) => (
                        <Input
                          {...input}
                          {...fieldProps}
                          autoComplete={field === "firstName" ? "given-name" : "family-name"}
                          maxLength={DISPLAY_TEXT_MAX}
                          onChange={(event) => {
                            setSaved(false);
                            input.onChange(event);
                          }}
                        />
                      )}
                    />
                  )}
                </FormField>
              </div>
            ))}
          </Flex>
          <Typography.Paragraph type="secondary" style={{ marginTop: 0 }}>
            {t("profile.managedByAdmin")}
          </Typography.Paragraph>
          <Flex align="center" gap={token.marginSM} wrap>
            <Button
              type="primary"
              htmlType="submit"
              loading={isSubmitting}
              disabled={!isDirty && !isSubmitting}
            >
              {t("profile.save")}
            </Button>
            <Typography.Text type="success" role="status">
              {saved ? t("profile.saved") : null}
            </Typography.Text>
          </Flex>
        </Form>
      </form>
    </SettingsSection>
  );
}
