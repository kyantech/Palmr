"use client";

import { useEffect, useMemo, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { z } from "zod";

import { useAppInfo } from "@/contexts/app-info-context";
import { useShareContext } from "@/contexts/share-context";
import { useAdminConfigs } from "@/hooks/use-secure-configs";
import { bulkUpdateConfigs } from "@/http/endpoints";
import { Config, ConfigType, GroupFormData } from "../types";

const createSchemas = () => ({
  settingsSchema: z.object({
    configs: z.record(z.union([z.string(), z.number()]).transform((val) => String(val))),
  }),
});

export function useSettings() {
  const t = useTranslations();
  const { settingsSchema } = createSchemas();
  const [isLoading, setIsLoading] = useState(true);
  const [configs, setConfigs] = useState<Record<string, string>>({});
  const [groupedConfigs, setGroupedConfigs] = useState<Record<string, Config[]>>({});
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({
    general: true,
    email: true,
    security: true,
    storage: true,
  });
  const { refreshAppInfo } = useAppInfo();
  const { refreshShareContext } = useShareContext();

  const {
    configs: adminConfigsList,
    isLoading: configsLoading,
    error: configsError,
    isUnauthorized,
    reload: reloadConfigs,
  } = useAdminConfigs();

  const generalForm = useForm<GroupFormData>({ resolver: zodResolver(settingsSchema) });
  const emailForm = useForm<GroupFormData>({ resolver: zodResolver(settingsSchema) });
  const securityForm = useForm<GroupFormData>({ resolver: zodResolver(settingsSchema) });
  const storageForm = useForm<GroupFormData>({ resolver: zodResolver(settingsSchema) });

  const groupForms = useMemo(
    () => ({
      general: generalForm,
      email: emailForm,
      security: securityForm,
      storage: storageForm,
    }),
    [generalForm, emailForm, securityForm, storageForm]
  );

  type ValidGroup = keyof typeof groupForms;

  useEffect(() => {
    if (!configsLoading && adminConfigsList.length > 0) {
      const configsData = adminConfigsList.reduce((acc: Record<string, string>, config) => {
        acc[config.key] = config.value;
        return acc;
      }, {});

      const grouped = adminConfigsList.reduce((acc: Record<string, Config[]>, config) => {
        const group = config.group || "general";

        if (!acc[group]) acc[group] = [];

        acc[group].push({
          ...config,
          type: (config.type as ConfigType) || "text",
        });

        acc[group].sort((a, b) => {
          if (group === "general") {
            if (a.key === "appLogo") return -1;
            if (b.key === "appLogo") return 1;
          }

          if (group === "email") {
            const smtpOrder = [
              "smtpEnabled",
              "smtpHost",
              "smtpPort",
              "smtpSecure",
              "smtpNoAuth",
              "smtpUser",
              "smtpPass",
              "smtpFromName",
              "smtpFromEmail",
            ];

            const aIndex = smtpOrder.indexOf(a.key);
            const bIndex = smtpOrder.indexOf(b.key);

            if (aIndex !== -1 && bIndex !== -1) {
              return aIndex - bIndex;
            }
            if (aIndex !== -1) return -1;
            if (bIndex !== -1) return 1;
          }

          if (group === "auth-providers") {
            if (a.key === "authProvidersEnabled") return -1;
            if (b.key === "authProvidersEnabled") return 1;
          }

          return a.key.localeCompare(b.key);
        });

        return acc;
      }, {});

      setConfigs(configsData);
      setGroupedConfigs(grouped);

      Object.entries(grouped).forEach(([groupName, groupConfigs]) => {
        if (groupName === "auth-providers") {
          return;
        }

        if (groupName === "general" || groupName === "email" || groupName === "security" || groupName === "storage") {
          const group = groupName as ValidGroup;
          const groupConfigData = groupConfigs.reduce(
            (acc, config) => {
              acc[config.key] = configsData[config.key];
              return acc;
            },
            {} as Record<string, string>
          );

          groupForms[group].reset({ configs: groupConfigData });
        }
      });

      setIsLoading(false);
    }
  }, [configsLoading, adminConfigsList, groupForms]);

  const onGroupSubmit = async (group: ValidGroup, data: GroupFormData) => {
    try {
      const groupConfigKeys = groupedConfigs[group].map((config) => config.key);
      const configsToUpdate = Object.entries(data.configs)
        .filter(([key, newValue]) => {
          const currentValue = configs[key];

          return groupConfigKeys.includes(key) && String(newValue) !== currentValue;
        })
        .map(([key, value]) => ({
          key,
          value: String(value),
        }));

      if (configsToUpdate.length === 0) {
        toast.info(t("settings.messages.noChanges"));

        return;
      }

      await bulkUpdateConfigs(configsToUpdate);
      toast.success(t("settings.messages.updateSuccess", { group: t(`settings.groups.${group}.title`) }));

      await reloadConfigs();

      if (group === "email") {
        await refreshShareContext();
      }

      await refreshAppInfo();
    } catch {
      toast.error(t("settings.errors.updateFailed"));
    }
  };

  const toggleCollapse = (group: string) => {
    setCollapsedGroups((prev) => ({
      ...prev,
      [group]: !prev[group],
    }));
  };

  return {
    isLoading,
    groupedConfigs,
    collapsedGroups,
    groupForms,
    toggleCollapse,
    onGroupSubmit,
    error: configsError,
    isUnauthorized,
  };
}
