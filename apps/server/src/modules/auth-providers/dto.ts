import { z } from "zod";

export const BaseAuthProviderSchema = z.object({
  name: z.string().min(1, "Name is required").describe("Provider name"),
  displayName: z.string().min(1, "Display name is required").describe("Provider display name"),
  type: z.enum(["oidc", "oauth2"]).describe("Provider type"),
  icon: z.string().optional().describe("Provider icon"),
  enabled: z.boolean().default(false).describe("Whether provider is enabled"),
  autoRegister: z.boolean().default(true).describe("Auto-register new users"),
  scope: z.string().optional().describe("OAuth scopes"),
  adminEmailDomains: z.string().optional().describe("Admin email domains (comma-separated)"),
  clientId: z.string().min(1, "Client ID is required").describe("OAuth client ID"),
  clientSecret: z.string().min(1, "Client secret is required").describe("OAuth client secret"),
});

export const DiscoveryModeSchema = BaseAuthProviderSchema.extend({
  issuerUrl: z.string().url("Invalid issuer URL").describe("Provider issuer URL for discovery"),
  authorizationEndpoint: z.literal("").optional(),
  tokenEndpoint: z.literal("").optional(),
  userInfoEndpoint: z.literal("").optional(),
});

export const ManualEndpointsSchema = BaseAuthProviderSchema.extend({
  issuerUrl: z.string().optional(),
  authorizationEndpoint: z
    .string()
    .min(1, "Authorization endpoint is required")
    .describe("Authorization endpoint URL or path"),
  tokenEndpoint: z.string().min(1, "Token endpoint is required").describe("Token endpoint URL or path"),
  userInfoEndpoint: z.string().min(1, "User info endpoint is required").describe("User info endpoint URL or path"),
});

export const CreateAuthProviderSchema = BaseAuthProviderSchema.extend({
  issuerUrl: z.string().url("Invalid issuer URL").optional(),
  authorizationEndpoint: z.string().optional(),
  tokenEndpoint: z.string().optional(),
  userInfoEndpoint: z.string().optional(),
}).refine(
  (data) => {
    const hasIssuerUrl = !!data.issuerUrl;
    const hasAnyCustomEndpoint = !!(
      data.authorizationEndpoint?.trim() ||
      data.tokenEndpoint?.trim() ||
      data.userInfoEndpoint?.trim()
    );

    if (hasIssuerUrl && !hasAnyCustomEndpoint) return true;

    if (hasAnyCustomEndpoint) {
      const hasAllCustomEndpoints = !!(
        data.authorizationEndpoint?.trim() &&
        data.tokenEndpoint?.trim() &&
        data.userInfoEndpoint?.trim()
      );
      return hasAllCustomEndpoints;
    }

    return false;
  },
  {
    message:
      "Either provide issuerUrl for automatic discovery OR all three custom endpoints (authorization, token, userInfo).",
  }
);

export const UpdateAuthProviderSchema = z
  .object({
    name: z.string().min(1).optional(),
    displayName: z.string().min(1).optional(),
    type: z.enum(["oidc", "oauth2"]).optional(),
    icon: z.string().optional(),
    enabled: z.boolean().optional(),
    autoRegister: z.boolean().optional(),
    scope: z.string().optional(),
    adminEmailDomains: z.string().optional(),
    clientId: z.string().min(1).optional(),
    clientSecret: z.string().min(1).optional(),
    issuerUrl: z.string().url().optional(),
    authorizationEndpoint: z.string().optional(),
    tokenEndpoint: z.string().optional(),
    userInfoEndpoint: z.string().optional(),
  })
  .refine(
    (data) => {
      const hasIssuerUrl = !!data.issuerUrl;
      const hasAnyCustomEndpoint = !!(
        data.authorizationEndpoint?.trim() ||
        data.tokenEndpoint?.trim() ||
        data.userInfoEndpoint?.trim()
      );

      if (!hasIssuerUrl && !hasAnyCustomEndpoint) return true;

      if (hasIssuerUrl && !hasAnyCustomEndpoint) return true;

      if (hasAnyCustomEndpoint) {
        const hasAllCustomEndpoints = !!(
          data.authorizationEndpoint?.trim() &&
          data.tokenEndpoint?.trim() &&
          data.userInfoEndpoint?.trim()
        );
        return hasAllCustomEndpoints;
      }

      return true;
    },
    {
      message: "When providing custom endpoints, all three endpoints (authorization, token, userInfo) are required.",
    }
  );

export const UpdateOfficialProviderSchema = z.object({
  issuerUrl: z.string().url().optional(),
  clientId: z.string().min(1).optional(),
  clientSecret: z.string().min(1).optional(),
  enabled: z.boolean().optional(),
  autoRegister: z.boolean().optional(),
  adminEmailDomains: z.string().optional(),
});

export const UpdateProvidersOrderSchema = z.object({
  providers: z.array(
    z.object({
      id: z.string(),
      sortOrder: z.number(),
    })
  ),
});
