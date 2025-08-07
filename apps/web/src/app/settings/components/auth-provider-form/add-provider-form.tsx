"use client";

import React, { useState } from "react";
import { IconInfoCircle, IconPlus } from "@tabler/icons-react";
import { useTranslations } from "next-intl";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { IconPicker } from "@/components/ui/icon-picker";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { TagsInput } from "@/components/ui/tags-input";
import type { NewProvider } from "@/http/endpoints/auth/types";
import { CallbackUrlDisplay } from "./callback-url-display";
import { ConfigurationMethodSelector } from "./configuration-method-selector";

interface AddProviderFormProps {
  showAddForm: boolean;
  onToggleForm: () => void;
  onAddProvider: (provider: NewProvider) => Promise<void>;
  saving: boolean;
}

export function AddProviderForm({ showAddForm, onToggleForm, onAddProvider, saving }: AddProviderFormProps) {
  const t = useTranslations();
  const [newProvider, setNewProvider] = useState<NewProvider>({
    name: "",
    displayName: "",
    type: "oidc",
    icon: "",
    clientId: "",
    clientSecret: "",
    issuerUrl: "",
    scope: "openid profile email",
    authorizationEndpoint: "",
    tokenEndpoint: "",
    userInfoEndpoint: "",
  });

  const detectProviderTypeAndSuggestScopes = (url: string): string[] => {
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
      { pattern: "authentik", scopes: ["openid", "profile", "email"] },
      { pattern: "keycloak", scopes: ["openid", "profile", "email"] },
      { pattern: "auth0.com", scopes: ["openid", "profile", "email"] },
      { pattern: "okta.com", scopes: ["openid", "profile", "email"] },
      { pattern: "onelogin.com", scopes: ["openid", "profile", "email"] },
      { pattern: "pingidentity.com", scopes: ["openid", "profile", "email"] },
      { pattern: "azure.com", scopes: ["openid", "profile", "email", "User.Read"] },
      { pattern: "aws.amazon.com", scopes: ["openid", "profile", "email"] },
      { pattern: "slack.com", scopes: ["identity.basic", "identity.email", "identity.avatar"] },
      { pattern: "bitbucket.org", scopes: ["account", "repository"] },
      { pattern: "atlassian.com", scopes: ["read:jira-user", "read:jira-work"] },
      { pattern: "salesforce.com", scopes: ["api", "refresh_token"] },
      { pattern: "zendesk.com", scopes: ["read"] },
      { pattern: "shopify.com", scopes: ["read_products", "read_customers"] },
      { pattern: "stripe.com", scopes: ["read"] },
      { pattern: "twilio.com", scopes: ["read"] },
      { pattern: "sendgrid.com", scopes: ["mail.send"] },
      { pattern: "mailchimp.com", scopes: ["read"] },
      { pattern: "hubspot.com", scopes: ["contacts", "crm.objects.contacts.read"] },
      { pattern: "zoom.us", scopes: ["user:read:admin"] },
      { pattern: "teams.microsoft.com", scopes: ["openid", "profile", "email", "User.Read"] },
      { pattern: "notion.so", scopes: ["read"] },
      { pattern: "figma.com", scopes: ["files:read"] },
      { pattern: "dropbox.com", scopes: ["files.content.read"] },
      { pattern: "box.com", scopes: ["root_readwrite"] },
      { pattern: "trello.com", scopes: ["read"] },
      { pattern: "asana.com", scopes: ["default"] },
      { pattern: "monday.com", scopes: ["read"] },
      { pattern: "clickup.com", scopes: ["read"] },
      { pattern: "linear.app", scopes: ["read"] },
      { pattern: "kinde.com", scopes: ["openid", "profile", "email"] },
      { pattern: "zitadel.com", scopes: ["openid", "profile", "email"] },
      { pattern: "pocketid", scopes: ["openid", "profile", "email"] },
    ];

    for (const { pattern, scopes } of providerPatterns) {
      if (urlLower.includes(pattern)) {
        return scopes;
      }
    }

    if (newProvider.type === "oidc") {
      return ["openid", "profile", "email"];
    } else {
      return ["profile", "email"];
    }
  };

  const updateProviderUrl = (url: string) => {
    if (!url.trim()) return;

    const suggestedScopes = detectProviderTypeAndSuggestScopes(url);

    setNewProvider((prev) => {
      const shouldUpdateScopes = !prev.scope || prev.scope === "openid profile email" || prev.scope === "profile email";

      return {
        ...prev,
        scope: shouldUpdateScopes ? suggestedScopes.join(" ") : prev.scope,
      };
    });
  };

  const handleSubmit = async () => {
    if (!newProvider.name || !newProvider.displayName || !newProvider.clientId || !newProvider.clientSecret) {
      toast.error(t("authProviders.messages.fillRequiredFields"));
      return;
    }

    const hasIssuerUrl = !!newProvider.issuerUrl;
    const hasAllCustomEndpoints = !!(
      newProvider.authorizationEndpoint &&
      newProvider.tokenEndpoint &&
      newProvider.userInfoEndpoint
    );

    if (!hasIssuerUrl && !hasAllCustomEndpoints) {
      toast.error(t("authProviders.messages.provideUrlOrEndpoints"));
      return;
    }

    if (hasIssuerUrl && hasAllCustomEndpoints) {
      toast.error(t("authProviders.messages.chooseDiscoveryOrManual"));
      return;
    }

    await onAddProvider(newProvider);

    setNewProvider({
      name: "",
      displayName: "",
      type: "oidc",
      icon: "",
      clientId: "",
      clientSecret: "",
      issuerUrl: "",
      scope: "openid profile email",
      authorizationEndpoint: "",
      tokenEndpoint: "",
      userInfoEndpoint: "",
    });
  };

  const updateProvider = (updates: Partial<NewProvider>) => {
    setNewProvider((prev) => ({ ...prev, ...updates }));
  };

  if (!showAddForm) {
    return (
      <Button onClick={onToggleForm} variant="outline" size="sm">
        <IconPlus className="h-4 w-4" />
        {t("authProviders.addProvider")}
      </Button>
    );
  }

  return (
    <div className="border border-dashed rounded-lg p-4 space-y-4">
      <div className="flex items-center justify-between">
        <h3 className="font-medium text-foreground dark:text-foreground">{t("authProviders.addProviderTitle")}</h3>
      </div>

      <div className="bg-blue-50 dark:bg-blue-950/50 border border-blue-200 dark:border-blue-800 rounded-lg p-3">
        <div className="flex items-center gap-2 text-blue-700 dark:text-blue-300">
          <span>
            <IconInfoCircle className="h-4 w-4" />
          </span>
          <span className="text-sm font-medium">{t("authProviders.info.title")}</span>
        </div>
        <p className="text-xs text-muted-foreground mt-1">
          {t("authProviders.info.officialProvidersRecommended")}{" "}
          <a
            href="https://github.com/kyantech/Palmr/issues"
            target="_blank"
            rel="noopener noreferrer"
            className="font-semibold"
          >
            {t("authProviders.info.github")}.
          </a>
        </p>
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div>
          <Label className="mb-2 block">{t("authProviders.form.providerName")} *</Label>
          <Input
            placeholder={t("authProviders.form.providerNamePlaceholder")}
            value={newProvider.name}
            onChange={(e) => updateProvider({ name: e.target.value })}
          />
        </div>
        <div>
          <Label className="mb-2 block">{t("authProviders.form.displayName")} *</Label>
          <Input
            placeholder={t("authProviders.form.displayNamePlaceholder")}
            value={newProvider.displayName}
            onChange={(e) => updateProvider({ displayName: e.target.value })}
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div>
          <Label className="mb-2 block">{t("authProviders.form.type")}</Label>
          <select
            className="w-full rounded-md border border-input bg-background dark:bg-background px-3 py-2 text-sm text-foreground dark:text-foreground"
            value={newProvider.type}
            onChange={(e) => updateProvider({ type: e.target.value as "oidc" | "oauth2" })}
          >
            <option value="oidc">{t("authProviders.form.typeOidc")}</option>
            <option value="oauth2">{t("authProviders.form.typeOauth2")}</option>
          </select>
        </div>
        <div>
          <Label className="mb-2 block">{t("authProviders.form.icon")}</Label>
          <IconPicker
            value={newProvider.icon}
            onChange={(icon) => updateProvider({ icon })}
            placeholder={t("authProviders.form.iconPlaceholder")}
          />
        </div>
      </div>

      <ConfigurationMethodSelector provider={newProvider} onUpdate={updateProvider} onUrlUpdate={updateProviderUrl} />

      <div className="grid grid-cols-2 gap-4">
        <div>
          <Label className="mb-2 block">{t("authProviders.form.clientId")} *</Label>
          <Input
            placeholder={t("authProviders.form.clientIdPlaceholder")}
            value={newProvider.clientId}
            onChange={(e) => updateProvider({ clientId: e.target.value })}
          />
        </div>
        <div>
          <Label className="mb-2 block">{t("authProviders.form.clientSecret")} *</Label>
          <Input
            type="password"
            placeholder={t("authProviders.form.clientSecretPlaceholder")}
            value={newProvider.clientSecret}
            onChange={(e) => updateProvider({ clientSecret: e.target.value })}
          />
        </div>
      </div>

      <div>
        <Label className="mb-2 block">{t("authProviders.form.oauthScopes")}</Label>
        <TagsInput
          value={newProvider.scope ? newProvider.scope.split(/[,\s]+/).filter(Boolean) : []}
          onChange={(tags) => updateProvider({ scope: tags.join(" ") })}
          placeholder={t("authProviders.form.scopesPlaceholder")}
        />
        <p className="text-xs text-muted-foreground mt-1">
          {newProvider.type === "oidc"
            ? t("authProviders.form.scopesHelpOidc")
            : t("authProviders.form.scopesHelpOauth2")}
        </p>
      </div>

      {newProvider.name && (
        <div className="pt-2">
          <CallbackUrlDisplay providerName={newProvider.name} />
        </div>
      )}

      <div className="flex gap-2 justify-end">
        <Button variant="outline" onClick={onToggleForm} size="sm">
          {t("authProviders.buttons.cancel")}
        </Button>
        <Button onClick={handleSubmit} disabled={saving} size="sm">
          {saving ? t("authProviders.buttons.adding") : t("authProviders.addProvider")}
        </Button>
      </div>
    </div>
  );
}
