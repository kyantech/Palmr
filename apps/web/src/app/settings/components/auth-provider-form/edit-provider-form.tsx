"use client";

import React, { useState } from "react";
import { IconEye, IconEyeOff, IconInfoCircle } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { Button } from "@/components/ui/button";
import { IconPicker } from "@/components/ui/icon-picker";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { TagsInput } from "@/components/ui/tags-input";
import { CallbackUrlDisplay } from "./callback-url-display";

export interface AuthProvider {
  id: string;
  name: string;
  displayName: string;
  type: string;
  icon?: string;
  enabled: boolean;
  issuerUrl?: string;
  clientId?: string;
  clientSecret?: string;
  scope?: string;
  autoRegister: boolean;
  adminEmailDomains?: string;
  sortOrder: number;
  isOfficial?: boolean;
  authorizationEndpoint?: string;
  tokenEndpoint?: string;
  userInfoEndpoint?: string;
}

interface EditProviderFormProps {
  provider: AuthProvider;
  onSave: (data: Partial<AuthProvider>) => void;
  onCancel: () => void;
  saving: boolean;
  editingFormData: Record<string, any>;
  setEditingFormData: (data: Record<string, any>) => void;
}

export function EditProviderForm({
  provider,
  onSave,
  onCancel,
  saving,
  editingFormData,
  setEditingFormData,
}: EditProviderFormProps) {
  const t = useTranslations();
  const savedData = editingFormData[provider.id] || {};
  const [formData, setFormData] = useState({
    name: savedData.name || provider.name || "",
    displayName: savedData.displayName || provider.displayName || "",
    type: (savedData.type || provider.type) as "oidc" | "oauth2",
    icon: savedData.icon || provider.icon || "FaCog",
    issuerUrl: savedData.issuerUrl || provider.issuerUrl || "",
    clientId: savedData.clientId || provider.clientId || "",
    clientSecret: savedData.clientSecret || provider.clientSecret || "",
    scope: savedData.scope || provider.scope || "",
    autoRegister: savedData.autoRegister !== undefined ? savedData.autoRegister : provider.autoRegister,
    adminEmailDomains: savedData.adminEmailDomains || provider.adminEmailDomains || "",
    authorizationEndpoint: savedData.authorizationEndpoint || provider.authorizationEndpoint || "",
    tokenEndpoint: savedData.tokenEndpoint || provider.tokenEndpoint || "",
    userInfoEndpoint: savedData.userInfoEndpoint || provider.userInfoEndpoint || "",
  });

  const [showClientSecret, setShowClientSecret] = useState(false);
  const isOfficial = provider.isOfficial;

  const isProviderUrlEditable = (providerName: string): boolean => {
    const nonEditableProviders = ["google", "discord", "github"];
    return !nonEditableProviders.includes(providerName.toLowerCase());
  };

  const canEditProviderUrl = isProviderUrlEditable(provider.name);

  const updateFormData = (updates: Partial<typeof formData>) => {
    const newFormData = { ...formData, ...updates };
    setFormData(newFormData);

    setEditingFormData({
      ...editingFormData,
      [provider.id]: newFormData,
    });
  };

  const detectProviderTypeAndSuggestScopesEdit = (url: string, currentType: string): string[] => {
    if (!url) return [];

    const urlLower = url.toLowerCase();

    const providerPatterns = [
      { pattern: "frontegg.com", scopes: ["openid", "profile", "email"] },
      { pattern: "discord.com", scopes: ["identify", "email"] },
      { pattern: "github.com", scopes: ["read:user", "user:email"] },
      { pattern: "gitlab.com", scopes: ["read_user", "read_api"] },
      { pattern: "google.com", scopes: ["openid", "profile", "email"] },
      { pattern: "microsoft.com", scopes: ["openid", "profile", "email", "User.Read"] },
      { pattern: "facebook.com", scopes: ["public_profile", "email"] },
      { pattern: "twitter.com", scopes: ["tweet.read", "users.read"] },
      { pattern: "linkedin.com", scopes: ["r_liteprofile", "r_emailaddress"] },
      { pattern: "auth0.com", scopes: ["openid", "profile", "email"] },
      { pattern: "okta.com", scopes: ["openid", "profile", "email"] },
      { pattern: "authentik", scopes: ["openid", "profile", "email"] },
      { pattern: "kinde.com", scopes: ["openid", "profile", "email"] },
      { pattern: "zitadel.com", scopes: ["openid", "profile", "email"] },
      { pattern: "pocketid", scopes: ["openid", "profile", "email"] },
    ];

    for (const { pattern, scopes } of providerPatterns) {
      if (urlLower.includes(pattern)) {
        return scopes;
      }
    }

    if (currentType === "oidc") {
      return ["openid", "profile", "email"];
    } else {
      return ["profile", "email"];
    }
  };

  const updateProviderUrlEdit = (url: string) => {
    if (!url.trim()) return;

    if (isOfficial) {
      return;
    }

    const suggestedScopes = detectProviderTypeAndSuggestScopesEdit(url, formData.type);
    const shouldUpdateScopes =
      !formData.scope || formData.scope === "openid profile email" || formData.scope === "profile email";

    if (shouldUpdateScopes) {
      updateFormData({
        scope: suggestedScopes.join(" "),
      });
    }
  };

  const handleSubmit = () => {
    onSave(formData);
  };

  const isManualMode = !!(formData.authorizationEndpoint || formData.tokenEndpoint || formData.userInfoEndpoint);

  return (
    <div className="space-y-4">
      {isOfficial && (
        <div className="bg-blue-50 dark:bg-blue-950/50 border border-blue-200 dark:border-blue-800 rounded-lg p-3">
          <div className="flex items-center gap-2 text-blue-700 dark:text-blue-300">
            <span>
              <IconInfoCircle className="h-4 w-4" />
            </span>
            <span className="text-sm font-medium">{t("authProviders.info.officialProvider")}</span>
          </div>
          <p className="text-xs text-blue-600 dark:text-blue-400 mt-1">
            {t("authProviders.info.officialProviderDescription")}
          </p>
        </div>
      )}

      <CallbackUrlDisplay providerName={formData.name || "provider"} />

      {!isOfficial && (
        <div className="grid grid-cols-2 gap-4">
          <div>
            <Label className="mb-2 block">{t("authProviders.form.providerName")} *</Label>
            <Input
              placeholder={t("authProviders.form.providerNamePlaceholder")}
              value={formData.name}
              onChange={(e) => updateFormData({ name: e.target.value })}
            />
          </div>
          <div>
            <Label className="mb-2 block">{t("authProviders.form.displayName")} *</Label>
            <Input
              placeholder={t("authProviders.form.displayNamePlaceholder")}
              value={formData.displayName}
              onChange={(e) => updateFormData({ displayName: e.target.value })}
            />
          </div>
        </div>
      )}

      {!isOfficial && (
        <div className="grid grid-cols-2 gap-4">
          <div>
            <Label className="mb-2 block">{t("authProviders.form.type")}</Label>
            <select
              className="w-full rounded-md border border-input bg-background dark:bg-background px-3 py-2 text-sm text-foreground dark:text-foreground"
              value={formData.type}
              onChange={(e) => updateFormData({ type: e.target.value as "oidc" | "oauth2" })}
            >
              <option value="oidc">{t("authProviders.form.typeOidc")}</option>
              <option value="oauth2">{t("authProviders.form.typeOauth2")}</option>
            </select>
          </div>
          <div>
            <Label className="mb-2 block">{t("authProviders.form.icon")}</Label>
            <IconPicker
              value={formData.icon}
              onChange={(icon) => updateFormData({ icon })}
              placeholder={t("authProviders.form.iconPlaceholder")}
            />
          </div>
        </div>
      )}

      {!isOfficial && (
        <div className="space-y-4">
          <div className="bg-gray-50 dark:bg-gray-900/50 border border-gray-200 dark:border-gray-800 rounded-lg p-4">
            <h4 className="text-sm font-medium mb-3">{t("authProviders.form.configurationMethod")}</h4>
            <div className="space-y-3">
              <div className="flex items-center space-x-2">
                <input
                  type="radio"
                  id="auto-discovery"
                  name="configMethod"
                  checked={!isManualMode}
                  onChange={() =>
                    updateFormData({
                      authorizationEndpoint: "",
                      tokenEndpoint: "",
                      userInfoEndpoint: "",
                    })
                  }
                  className="w-4 h-4"
                />
                <label htmlFor="auto-discovery" className="text-sm">
                  <span className="font-medium">{t("authProviders.form.autoDiscovery")}</span>
                  <span className="text-muted-foreground ml-2">
                    ({t("authProviders.form.autoDiscoveryDescription")})
                  </span>
                </label>
              </div>
              <div className="flex items-center space-x-2">
                <input
                  type="radio"
                  id="manual-endpoints"
                  name="configMethod"
                  checked={isManualMode}
                  onChange={() => {
                    if (!isManualMode) {
                      updateFormData({
                        authorizationEndpoint: "/oauth/authorize",
                        tokenEndpoint: "/oauth/token",
                        userInfoEndpoint: "/oauth/userinfo",
                      });
                    }
                  }}
                  className="w-4 h-4"
                />
                <label htmlFor="manual-endpoints" className="text-sm">
                  <span className="font-medium">{t("authProviders.form.manualEndpoints")}</span>
                  <span className="text-muted-foreground ml-2">
                    ({t("authProviders.form.manualEndpointsDescription")})
                  </span>
                </label>
              </div>
            </div>
          </div>

          {!isManualMode && (
            <div>
              <Label className="mb-2 block">{t("authProviders.form.providerUrl")} *</Label>
              <Input
                placeholder={t("authProviders.form.providerUrlAutoPlaceholder")}
                value={formData.issuerUrl}
                onChange={(e) => updateFormData({ issuerUrl: e.target.value })}
                onBlur={(e) => updateProviderUrlEdit(e.target.value)}
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
                  value={formData.issuerUrl}
                  onChange={(e) => updateFormData({ issuerUrl: e.target.value })}
                  onBlur={(e) => updateProviderUrlEdit(e.target.value)}
                />
                <p className="text-xs text-muted-foreground mt-1">{t("authProviders.form.manualConfigurationHelp")}</p>
              </div>
              <div>
                <Label className="mb-2 block">{t("authProviders.form.authorizationEndpoint")} *</Label>
                <Input
                  placeholder={t("authProviders.form.authorizationEndpointPlaceholder")}
                  value={formData.authorizationEndpoint}
                  onChange={(e) => updateFormData({ authorizationEndpoint: e.target.value })}
                />
              </div>
              <div>
                <Label className="mb-2 block">{t("authProviders.form.tokenEndpoint")} *</Label>
                <Input
                  placeholder={t("authProviders.form.tokenEndpointPlaceholder")}
                  value={formData.tokenEndpoint}
                  onChange={(e) => updateFormData({ tokenEndpoint: e.target.value })}
                />
              </div>
              <div>
                <Label className="mb-2 block">{t("authProviders.form.userInfoEndpoint")} *</Label>
                <Input
                  placeholder={t("authProviders.form.userInfoEndpointPlaceholder")}
                  value={formData.userInfoEndpoint}
                  onChange={(e) => updateFormData({ userInfoEndpoint: e.target.value })}
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
      )}

      {isOfficial && (
        <div className="space-y-4">
          {canEditProviderUrl ? (
            <div>
              <Label className="mb-2 block">{t("authProviders.form.providerUrl")} *</Label>
              <Input
                placeholder={t("authProviders.form.officialProviderUrlPlaceholder", {
                  displayName: provider.displayName,
                })}
                value={formData.issuerUrl}
                onChange={(e) => updateFormData({ issuerUrl: e.target.value })}
                onBlur={(e) => updateProviderUrlEdit(e.target.value)}
              />
              <p className="text-xs text-muted-foreground mt-1">{t("authProviders.form.officialProviderHelp")}</p>
            </div>
          ) : null}
          <div>
            <Label className="mb-2 block">{t("authProviders.form.icon")}</Label>
            <IconPicker
              value={formData.icon}
              onChange={(icon) => updateFormData({ icon })}
              placeholder={t("authProviders.form.iconPlaceholder")}
            />
            <p className="text-xs text-muted-foreground mt-1">{t("authProviders.form.officialProviderIconHelp")}</p>
          </div>
        </div>
      )}

      <div className="grid grid-cols-2 gap-4">
        <div>
          <Label className="mb-2 block">{t("authProviders.form.clientId")} *</Label>
          <Input
            placeholder={t("authProviders.form.clientIdPlaceholder")}
            value={formData.clientId}
            onChange={(e) => updateFormData({ clientId: e.target.value })}
          />
        </div>
        <div>
          <Label className="mb-2 block">{t("authProviders.form.clientSecret")} *</Label>
          <div className="relative">
            <Input
              type={showClientSecret ? "text" : "password"}
              placeholder={t("authProviders.form.clientSecretPlaceholder")}
              value={formData.clientSecret}
              onChange={(e) => updateFormData({ clientSecret: e.target.value })}
              className="pr-10"
            />
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="absolute right-0 top-0 h-full px-3 py-2 hover:bg-transparent"
              onClick={() => setShowClientSecret(!showClientSecret)}
            >
              {showClientSecret ? (
                <IconEyeOff className="h-4 w-4 text-muted-foreground" />
              ) : (
                <IconEye className="h-4 w-4 text-muted-foreground" />
              )}
            </Button>
          </div>
        </div>
      </div>

      <div>
        <Label className="mb-2 block">{t("authProviders.form.oauthScopes")}</Label>
        <TagsInput
          value={formData.scope ? formData.scope.split(/[,\s]+/).filter(Boolean) : []}
          onChange={(tags) => updateFormData({ scope: tags.join(" ") })}
          placeholder={t("authProviders.form.scopesPlaceholder")}
        />
        <p className="text-xs text-muted-foreground mt-1">
          {formData.type === "oidc" ? t("authProviders.form.scopesHelpOidc") : t("authProviders.form.scopesHelpOauth2")}
        </p>
      </div>

      <div>
        <Label className="mb-2 block">{t("authProviders.form.adminEmailDomains")}</Label>
        <TagsInput
          value={formData.adminEmailDomains ? formData.adminEmailDomains.split(",").filter(Boolean) : []}
          onChange={(tags) => updateFormData({ adminEmailDomains: tags.join(",") })}
          placeholder={t("authProviders.form.adminEmailDomainsPlaceholder")}
        />
        <p className="text-xs text-muted-foreground mt-1">{t("authProviders.form.adminEmailDomainsHelp")}</p>
      </div>

      <div className="flex items-center gap-2">
        <Switch
          checked={formData.autoRegister}
          onCheckedChange={(checked) => updateFormData({ autoRegister: checked })}
        />
        <Label className="cursor-pointer">{t("authProviders.form.autoRegister")}</Label>
      </div>

      <div className="flex gap-2 justify-end pt-4">
        <Button variant="outline" onClick={onCancel} size="sm">
          {t("authProviders.buttons.cancel")}
        </Button>
        <Button onClick={handleSubmit} disabled={saving} size="sm">
          {saving ? t("authProviders.buttons.saving") : t("authProviders.buttons.saveProvider")}
        </Button>
      </div>
    </div>
  );
}
