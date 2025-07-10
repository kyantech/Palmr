"use client";

import React from "react";
import { IconInfoCircle } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { NewProvider } from "@/http/endpoints/auth/types";

interface ConfigurationMethodSelectorProps {
  provider: NewProvider;
  onUpdate: (updates: Partial<NewProvider>) => void;
  onUrlUpdate: (url: string) => void;
}

export function ConfigurationMethodSelector({ provider, onUpdate, onUrlUpdate }: ConfigurationMethodSelectorProps) {
  const t = useTranslations();
  const isManualMode = !!(provider.authorizationEndpoint || provider.tokenEndpoint || provider.userInfoEndpoint);

  return (
    <div className="space-y-4">
      <div className="bg-gray-50 dark:bg-gray-900/50 border border-gray-200 dark:border-gray-800 rounded-lg p-4">
        <h4 className="text-sm font-medium mb-3">{t("authProviders.form.configurationMethod")}</h4>
        <div className="space-y-3">
          <div className="flex items-center space-x-2">
            <input
              type="radio"
              id="add-auto-discovery"
              name="addConfigMethod"
              checked={!isManualMode}
              onChange={() =>
                onUpdate({
                  authorizationEndpoint: "",
                  tokenEndpoint: "",
                  userInfoEndpoint: "",
                })
              }
              className="w-4 h-4"
            />
            <label htmlFor="add-auto-discovery" className="text-sm">
              <span className="font-medium">{t("authProviders.form.autoDiscovery")}</span>
              <span className="text-muted-foreground ml-2">({t("authProviders.form.autoDiscoveryDescription")})</span>
            </label>
          </div>
          <div className="flex items-center space-x-2">
            <input
              type="radio"
              id="add-manual-endpoints"
              name="addConfigMethod"
              checked={isManualMode}
              onChange={() => {
                if (!isManualMode) {
                  onUpdate({
                    authorizationEndpoint: "/oauth/authorize",
                    tokenEndpoint: "/oauth/token",
                    userInfoEndpoint: "/oauth/userinfo",
                    issuerUrl: "",
                  });
                }
              }}
              className="w-4 h-4"
            />
            <label htmlFor="add-manual-endpoints" className="text-sm">
              <span className="font-medium">{t("authProviders.form.manualEndpoints")}</span>
              <span className="text-muted-foreground ml-2">({t("authProviders.form.manualEndpointsDescription")})</span>
            </label>
          </div>
        </div>
      </div>

      {!isManualMode && (
        <div>
          <Label className="mb-2 block">{t("authProviders.form.providerUrl")} *</Label>
          <Input
            placeholder={t("authProviders.form.providerUrlAutoPlaceholder")}
            value={provider.issuerUrl}
            onChange={(e) => onUpdate({ issuerUrl: e.target.value })}
            onBlur={(e) => onUrlUpdate(e.target.value)}
          />
          <p className="text-xs text-muted-foreground mt-1">{t("authProviders.form.autoDiscoveryHelp")}</p>
        </div>
      )}

      {isManualMode && (
        <div className="space-y-4">
          <div>
            <Label className="mb-2 block">{t("authProviders.form.providerUrl")} *</Label>
            <Input
              placeholder={t("authProviders.form.providerUrlManualPlaceholder")}
              value={provider.issuerUrl}
              onChange={(e) => onUpdate({ issuerUrl: e.target.value })}
              onBlur={(e) => onUrlUpdate(e.target.value)}
            />
            <p className="text-xs text-muted-foreground mt-1">{t("authProviders.form.manualConfigurationHelp")}</p>
          </div>
          <div>
            <Label className="mb-2 block">{t("authProviders.form.authorizationEndpoint")} *</Label>
            <Input
              placeholder={t("authProviders.form.authorizationEndpointPlaceholder")}
              value={provider.authorizationEndpoint}
              onChange={(e) => onUpdate({ authorizationEndpoint: e.target.value })}
            />
          </div>
          <div>
            <Label className="mb-2 block">{t("authProviders.form.tokenEndpoint")} *</Label>
            <Input
              placeholder={t("authProviders.form.tokenEndpointPlaceholder")}
              value={provider.tokenEndpoint}
              onChange={(e) => onUpdate({ tokenEndpoint: e.target.value })}
            />
          </div>
          <div>
            <Label className="mb-2 block">{t("authProviders.form.userInfoEndpoint")} *</Label>
            <Input
              placeholder={t("authProviders.form.userInfoEndpointPlaceholder")}
              value={provider.userInfoEndpoint}
              onChange={(e) => onUpdate({ userInfoEndpoint: e.target.value })}
            />
          </div>
          <div className="bg-blue-50 dark:bg-blue-950/50 border border-blue-200 dark:border-blue-800 rounded-lg p-3">
            <div className="flex items-start gap-2 text-blue-700 dark:text-blue-300">
              <IconInfoCircle className="h-4 w-4 mt-0.5 flex-shrink-0" />
              <div className="text-xs">
                <p className="font-medium">{t("authProviders.info.manualConfigTitle")}</p>
                <p className="mt-1">{t("authProviders.info.manualConfigDescription")}</p>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
