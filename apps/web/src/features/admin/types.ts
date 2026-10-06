import type { components } from "../../shared/api/schema";

type Schemas = components["schemas"];

export type UserRow = Schemas["AdminUserItem"];
export type UserDetail = Schemas["AdminUserDetail"];
export type UserPage = Schemas["Page_AdminUserItem"];
export type UserQuota = Schemas["AdminUserQuota"];
export type UserSessionPage = Schemas["Page_SessionItem"];
export type UserSession = Schemas["SessionItem"];
export type IdentityLink = Schemas["AdminIdentityLink"];
export type CreateUserRequest = Schemas["CreateUserRequest"];
export type UpdateUserRequest = Schemas["UpdateUserRequest"];
export type QuotaOverrideRequest = Schemas["QuotaOverrideRequest"];
export type PasswordReset = Schemas["AdminPasswordReset"];

export type InviteItem = Schemas["InviteItem"];
export type InvitePage = Schemas["Page_InviteItem"];
export type InviteStatus = Schemas["InviteStatus"];
export type CreateInviteRequest = Schemas["CreateInviteRequest"];
export type CreatedInvite = Schemas["CreateInviteResponse"];

export type GeneralSettings = Schemas["GeneralSettings"];
export type GeneralPatch = Schemas["GeneralPatch"];
export type SecuritySettings = Schemas["SecuritySettings"];
export type SecurityPatch = Schemas["SecurityPatch"];
export type QuotaSettings = Schemas["QuotaSettings"];
export type QuotaPatch = Schemas["QuotaPatch"];
export type PublicLinkSettings = Schemas["PublicLinkSettings"];
export type PublicLinkPatch = Schemas["PublicLinkPatch"];
export type SmtpSettings = Schemas["SmtpSettings"];
export type SmtpPatch = Schemas["SmtpPatch"];
export type SmtpSecurity = Schemas["SmtpSecurity"];
export type SmtpTestRequest = Schemas["SmtpTestRequest"];
export type SmtpTestResult = Schemas["SmtpTestResult"];
export type SmtpTestStage = Schemas["SmtpTestStage"];
export type SmtpUnsavedSettings = Schemas["SmtpUnsavedSettings"];
export type ThumbnailSourceLimit = Schemas["ThumbnailSourceLimitName"];

export type Provider = Schemas["ProviderItem"];
export type ProviderPage = Schemas["Page_ProviderItem"];
export type ProviderPreset = Schemas["PresetItem"];
export type PresetKey = Schemas["Preset"];
export type ProviderProtocol = Schemas["Protocol"];
export type TokenAuthMethod = Schemas["TokenAuthMethod"];
export type CreateProviderRequest = Schemas["CreateProviderRequest"];
export type UpdateProviderRequest = Schemas["UpdateProviderRequest"];
export type ProviderEndpoints = Schemas["Endpoints"];
export type ProviderClaimMapping = Schemas["ClaimMapping"];
export type DiscoveredProvider = Schemas["Discovered"];
export type ProviderTestResult = Schemas["ProviderTestResult"];
export type PasswordLoginState = Schemas["PasswordLoginState"];
export type PasswordLoginRequest = Schemas["PasswordLoginRequest"];

export const TOKEN_AUTH_METHODS = [
  "client_secret_basic",
  "client_secret_post",
  "none",
] as const satisfies readonly TokenAuthMethod[];

export const USER_ROLES = ["user", "admin"] as const;
export type UserRole = (typeof USER_ROLES)[number];

export const USER_STATUSES = ["active", "inactive"] as const;
export type UserStatus = (typeof USER_STATUSES)[number];

export const USER_SORTS = [
  "createdAt:desc",
  "createdAt:asc",
  "username:asc",
  "username:desc",
  "email:asc",
  "email:desc",
  "usedBytes:desc",
  "usedBytes:asc",
] as const;
export type UserSort = (typeof USER_SORTS)[number];

export const INVITE_STATUSES = [
  "pending",
  "accepted",
  "revoked",
  "expired",
] as const satisfies readonly InviteStatus[];

export const THUMBNAIL_LIMITS = [
  "64MiB",
  "128MiB",
  "256MiB",
  "512MiB",
  "unlimited",
] as const satisfies readonly ThumbnailSourceLimit[];

export const SMTP_SECURITY_MODES = [
  "starttls",
  "implicit",
  "none",
] as const satisfies readonly SmtpSecurity[];

export type QuotaMode = "inherit" | "unlimited" | "bytes";

export function isUserRole(value: string): value is UserRole {
  return (USER_ROLES as readonly string[]).includes(value);
}
