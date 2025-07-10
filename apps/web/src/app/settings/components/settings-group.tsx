import React from "react";
import { IconChevronDown, IconChevronUp, IconDeviceFloppy } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { createFieldDescriptions, createGroupMetadata } from "../constants";
import { SettingsGroupProps } from "../types";
import { isFieldHidden, SettingsInput } from "./settings-input";
import { SmtpTestButton } from "./smtp-test-button";

export function SettingsGroup({ group, configs, form, isCollapsed, onToggleCollapse, onSubmit }: SettingsGroupProps) {
  const t = useTranslations();
  const GROUP_METADATA = createGroupMetadata(t);
  const FIELD_DESCRIPTIONS = createFieldDescriptions(t);

  const metadata = GROUP_METADATA[group as keyof typeof GROUP_METADATA] || {
    title: group,
    description: t("settings.groups.defaultDescription"),
  };

  const isEmailGroup = group === "email";

  return (
    <form onSubmit={form.handleSubmit(onSubmit)}>
      <Card className="p-6 gap-0">
        <CardHeader
          className="flex flex-row items-center justify-between cursor-pointer p-0"
          onClick={onToggleCollapse}
        >
          <div className="flex flex-row items-center gap-8">
            {metadata.icon && React.createElement(metadata.icon, { className: "text-xl text-muted-foreground" })}
            <div className="flex flex-col gap-1">
              <h2 className="text-xl font-semibold">
                {t(`settings.groups.${group}.title`, { defaultValue: metadata.title })}
              </h2>
              <p className="text-sm text-muted-foreground">
                {t(`settings.groups.${group}.description`, { defaultValue: metadata.description })}
              </p>
            </div>
          </div>
          {isCollapsed ? (
            <IconChevronDown className="text-muted-foreground" />
          ) : (
            <IconChevronUp className="text-muted-foreground" />
          )}
        </CardHeader>
        <CardContent className={`${isCollapsed ? "hidden" : "block"} px-0`}>
          <Separator className="my-6" />
          <div className="flex flex-col gap-4">
            {configs
              .filter((config) => !isFieldHidden(config.key))
              .map((config) => {
                const smtpEnabled = form.watch("configs.smtpEnabled");
                const smtpNoAuth = form.watch("configs.smtpNoAuth");
                const isSmtpAuthField = config.key === "smtpUser" || config.key === "smtpPass";

                const smtpFields = [
                  "smtpHost",
                  "smtpPort",
                  "smtpUser",
                  "smtpPass",
                  "smtpSecure",
                  "smtpNoAuth",
                  "smtpTrustSelfSigned",
                  "smtpFromName",
                  "smtpFromEmail",
                ];

                if (smtpEnabled !== "true" && smtpFields.includes(config.key)) {
                  return null;
                }

                if (isSmtpAuthField && smtpNoAuth === "true") {
                  return null;
                }

                return (
                  <div key={config.key} className="space-y-2 mb-3">
                    <SettingsInput
                      config={config}
                      error={form.formState.errors.configs?.[config.key]}
                      register={form.register}
                      setValue={form.setValue}
                      smtpEnabled={form.watch("configs.smtpEnabled")}
                      authProvidersEnabled={form.watch("configs.authProvidersEnabled")}
                      watch={form.watch}
                    />
                    <p className="text-xs text-muted-foreground ml-1">
                      {t(`settings.fields.${config.key}.description`, {
                        defaultValue:
                          FIELD_DESCRIPTIONS[config.key as keyof typeof FIELD_DESCRIPTIONS] ||
                          config.description ||
                          t("settings.fields.noDescription"),
                      })}
                    </p>
                  </div>
                );
              })}
          </div>
          <div className="flex justify-between items-center mt-4">
            <div className="flex">
              {isEmailGroup && form.watch("configs.smtpEnabled") === "true" && (
                <SmtpTestButton
                  smtpEnabled={form.watch("configs.smtpEnabled") || "false"}
                  getFormValues={() => ({
                    smtpEnabled: form.getValues("configs.smtpEnabled") || "false",
                    smtpHost: form.getValues("configs.smtpHost") || "",
                    smtpPort: form.getValues("configs.smtpPort") || "",
                    smtpUser: form.getValues("configs.smtpUser") || "",
                    smtpPass: form.getValues("configs.smtpPass") || "",
                    smtpSecure: form.getValues("configs.smtpSecure") || "auto",
                    smtpNoAuth: form.getValues("configs.smtpNoAuth") || "false",
                    smtpTrustSelfSigned: form.getValues("configs.smtpTrustSelfSigned") || "false",
                  })}
                />
              )}
            </div>
            <div className="flex">
              <Button
                variant="default"
                disabled={form.formState.isSubmitting}
                className="flex items-center gap-2"
                type="submit"
              >
                {!form.formState.isSubmitting && <IconDeviceFloppy className="h-4 w-4" />}
                {t("settings.buttons.save", {
                  group: t(`settings.groups.${group}.title`, { defaultValue: metadata.title }),
                })}
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>
    </form>
  );
}
