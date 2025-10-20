import { IconDatabase, IconMail, IconSettings, IconShield, IconUserCheck } from "@tabler/icons-react";
import { createTranslator } from "next-intl";

export const createGroupMetadata = (t: ReturnType<typeof createTranslator>) => ({
  email: {
    title: t("settings.groups.email.title"),
    description: t("settings.groups.email.description"),
    icon: IconMail,
  },
  general: {
    title: t("settings.groups.general.title"),
    description: t("settings.groups.general.description"),
    icon: IconSettings,
  },
  "auth-providers": {
    title: "Authentication Providers",
    description: "Configure external authentication providers for SSO",
    icon: IconUserCheck,
  },
  security: {
    title: t("settings.groups.security.title"),
    description: t("settings.groups.security.description"),
    icon: IconShield,
  },
  storage: {
    title: t("settings.groups.storage.title"),
    description: t("settings.groups.storage.description"),
    icon: IconDatabase,
  },
});

export const createFieldDescriptions = (t: ReturnType<typeof createTranslator>) => ({
  // General settings
  appLogo: t("settings.fields.appLogo.description"),
  appName: t("settings.fields.appName.description"),
  appDescription: t("settings.fields.appDescription.description"),
  showHomePage: t("settings.fields.showHomePage.description"),
  hideVersion: t("settings.fields.hideVersion.description"),
  firstUserAccess: t("settings.fields.firstUserAccess.description"),
  serverUrl: t("settings.fields.serverUrl.description"),

  // Email settings
  smtpEnabled: t("settings.fields.smtpEnabled.description"),
  smtpHost: t("settings.fields.smtpHost.description"),
  smtpPort: t("settings.fields.smtpPort.description"),
  smtpUser: t("settings.fields.smtpUser.description"),
  smtpPass: t("settings.fields.smtpPass.description"),
  smtpFromName: t("settings.fields.smtpFromName.description"),
  smtpFromEmail: t("settings.fields.smtpFromEmail.description"),
  smtpSecure: t("settings.fields.smtpSecure.description"),
  smtpNoAuth: t("settings.fields.smtpNoAuth.description"),
  smtpTrustSelfSigned: t("settings.fields.smtpTrustSelfSigned.description"),

  // Auth Providers settings
  authProvidersEnabled: "Enable external authentication providers for SSO",

  // Security settings
  maxLoginAttempts: t("settings.fields.maxLoginAttempts.description"),
  loginBlockDuration: t("settings.fields.loginBlockDuration.description"),
  passwordMinLength: t("settings.fields.passwordMinLength.description"),
  passwordResetTokenExpiration: t("settings.fields.passwordResetTokenExpiration.description"),

  // Storage settings
  maxFileSize: t("settings.fields.maxFileSize.description"),
  maxTotalStoragePerUser: t("settings.fields.maxTotalStoragePerUser.description"),
});

export const createFieldTitles = (t: ReturnType<typeof createTranslator>) => ({
  // General settings
  appLogo: t("settings.fields.appLogo.title"),
  appName: t("settings.fields.appName.title"),
  appDescription: t("settings.fields.appDescription.title"),
  showHomePage: t("settings.fields.showHomePage.title"),
  hideVersion: t("settings.fields.hideVersion.title"),
  firstUserAccess: t("settings.fields.firstUserAccess.title"),
  serverUrl: t("settings.fields.serverUrl.title"),

  // Email settings
  smtpEnabled: t("settings.fields.smtpEnabled.title"),
  smtpHost: t("settings.fields.smtpHost.title"),
  smtpPort: t("settings.fields.smtpPort.title"),
  smtpUser: t("settings.fields.smtpUser.title"),
  smtpPass: t("settings.fields.smtpPass.title"),
  smtpFromName: t("settings.fields.smtpFromName.title"),
  smtpFromEmail: t("settings.fields.smtpFromEmail.title"),
  smtpSecure: t("settings.fields.smtpSecure.title"),
  smtpNoAuth: t("settings.fields.smtpNoAuth.title"),
  smtpTrustSelfSigned: t("settings.fields.smtpTrustSelfSigned.title"),

  // Auth Providers settings
  authProvidersEnabled: "Authentication Providers Enabled",

  // Security settings
  maxLoginAttempts: t("settings.fields.maxLoginAttempts.title"),
  loginBlockDuration: t("settings.fields.loginBlockDuration.title"),
  passwordMinLength: t("settings.fields.passwordMinLength.title"),
  passwordResetTokenExpiration: t("settings.fields.passwordResetTokenExpiration.title"),

  // Storage settings
  maxFileSize: t("settings.fields.maxFileSize.title"),
  maxTotalStoragePerUser: t("settings.fields.maxTotalStoragePerUser.title"),
});
