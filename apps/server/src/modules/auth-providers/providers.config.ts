import { ProviderConfig, ProvidersConfigFile } from "./types";

export const PROVIDER_PATTERNS = [
  { pattern: "frontegg.com", type: "frontegg" },
  { pattern: "discord.com", type: "discord" },
  { pattern: "github.com", type: "github" },
  { pattern: "gitlab.com", type: "gitlab" },
  { pattern: "google.com", type: "google" },
  { pattern: "microsoft.com", type: "microsoft" },
  { pattern: "authentik", type: "authentik" },
  { pattern: "keycloak", type: "keycloak" },
  { pattern: "auth0.com", type: "auth0" },
  { pattern: "okta.com", type: "okta" },
  { pattern: "kinde.com", type: "kinde" },
  { pattern: "zitadel.com", type: "zitadel" },
  { pattern: "pocketid", type: "pocketid" },
] as const;

export const DEFAULT_SCOPES_BY_TYPE: Record<string, string[]> = {
  frontegg: ["openid", "profile", "email"],
  discord: ["identify", "email"],
  github: ["read:user", "user:email"],
  gitlab: ["read_user", "read_api"],
  google: ["openid", "profile", "email"],
  microsoft: ["openid", "profile", "email", "User.Read"],
  authentik: ["openid", "profile", "email"],
  keycloak: ["openid", "profile", "email"],
  auth0: ["openid", "profile", "email"],
  okta: ["openid", "profile", "email"],
  kinde: ["openid", "profile", "email"],
  zitadel: ["openid", "profile", "email"],
  pocketid: ["openid", "profile", "email"],
} as const;

export const DISCOVERY_SUPPORTED_PROVIDERS = [
  "frontegg",
  "oidc",
  "authentik",
  "keycloak",
  "auth0",
  "okta",
  "google",
  "microsoft",
  "kinde",
  "zitadel",
  "pocketid",
] as const;

export const DISCOVERY_PATHS = [
  "/.well-known/openid_configuration",
  "/.well-known/openid-configuration",
  "/.well-known/oauth-authorization-server",
] as const;

export const FALLBACK_ENDPOINTS: Record<string, any> = {
  frontegg: {
    authorizationEndpoint: "/oauth/authorize",
    tokenEndpoint: "/oauth/token",
    userInfoEndpoint: "/api/oauth/userinfo",
  },
  github: {
    authorizationEndpoint: "/login/oauth/authorize",
    tokenEndpoint: "/login/oauth/access_token",
    userInfoEndpoint: "/user",
  },
  gitlab: {
    authorizationEndpoint: "/oauth/authorize",
    tokenEndpoint: "/oauth/token",
    userInfoEndpoint: "/api/v4/user",
  },
  discord: {
    authorizationEndpoint: "/oauth2/authorize",
    tokenEndpoint: "/oauth2/token",
    userInfoEndpoint: "/users/@me",
  },
  oidc: {
    authorizationEndpoint: "/oauth2/authorize",
    tokenEndpoint: "/oauth2/token",
    userInfoEndpoint: "/oauth2/userinfo",
  },
  pocketid: {
    authorizationEndpoint: "/authorize",
    tokenEndpoint: "/api/oidc/token",
    userInfoEndpoint: "/api/oidc/userinfo",
  },
} as const;

/**
 * Configuração técnica oficial do Discord
 * OAuth2 com mapeamentos específicos do Discord
 * Endpoints vêm do banco de dados
 */
const discordConfig: ProviderConfig = {
  supportsDiscovery: false,
  authMethod: "body",
  fieldMappings: {
    id: ["id"],
    email: ["email"],
    name: ["global_name", "username"],
    firstName: ["global_name"],
    lastName: [],
    avatar: ["avatar"],
  },
};

/**
 * Configuração técnica oficial do Google
 * OAuth2 com discovery automático
 * Endpoints vêm do banco de dados
 */
const googleConfig: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid_configuration",
  authMethod: "body",
  fieldMappings: {
    id: ["sub"],
    email: ["email"],
    name: ["name"],
    firstName: ["given_name"],
    lastName: ["family_name"],
    avatar: ["picture"],
  },
};

/**
 * Configuração técnica oficial do GitHub
 * OAuth2 com busca separada de email
 * Endpoints vêm do banco de dados
 */
const githubConfig: ProviderConfig = {
  supportsDiscovery: false,
  authMethod: "body",
  specialHandling: {
    emailEndpoint: "https://api.github.com/user/emails",
    emailFetchRequired: true,
    responseFormat: "json",
  },
  fieldMappings: {
    id: ["id", "login"],
    email: ["email"],
    name: ["name", "login"],
    firstName: ["name"],
    lastName: [],
    avatar: ["avatar_url"],
  },
};

/**
 * Configuração técnica oficial do Auth0
 * OIDC com discovery automático
 * Endpoints vêm do banco de dados
 */
const auth0Config: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid_configuration",
  authMethod: "body",
  fieldMappings: {
    id: ["sub"],
    email: ["email"],
    name: ["name"],
    firstName: ["given_name"],
    lastName: ["family_name"],
    avatar: ["picture"],
  },
};

/**
 * Configuração técnica oficial do Kinde
 * OIDC com mapeamentos de campo customizados
 * Endpoints vêm do banco de dados
 */
const kindeConfig: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid_configuration",
  authMethod: "body",
  fieldMappings: {
    id: ["id"],
    email: ["preferred_email"],
    name: ["first_name", "last_name"],
    firstName: ["first_name"],
    lastName: ["last_name"],
    avatar: ["picture"],
  },
};

/**
 * Configuração técnica oficial do Zitadel
 * OIDC com Basic Auth
 * Endpoints vêm do banco de dados
 */
const zitadelConfig: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid_configuration",
  authMethod: "basic",
  fieldMappings: {
    id: ["sub"],
    email: ["email"],
    name: ["name"],
    firstName: ["given_name"],
    lastName: ["family_name"],
    avatar: ["picture"],
  },
};

/**
 * Configuração técnica oficial do Authentik
 * OIDC self-hosted com discovery
 * Endpoints vêm do banco de dados
 */
const authentikConfig: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid_configuration",
  authMethod: "body",
  fieldMappings: {
    id: ["sub"],
    email: ["email"],
    name: ["name"],
    firstName: ["given_name"],
    lastName: ["family_name"],
    avatar: ["picture"],
  },
};

/**
 * Configuração técnica oficial do Frontegg
 * OIDC multi-tenant com discovery automático
 * Endpoints vêm do banco de dados
 */
const fronteggConfig: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid-configuration",
  authMethod: "body",
  fieldMappings: {
    id: ["sub", "id", "user_id"],
    email: ["email", "preferred_username"],
    name: ["name", "preferred_username"],
    firstName: ["given_name", "name"],
    lastName: ["family_name"],
    avatar: ["picture"],
  },
};

/**
 * Configuração técnica oficial do Pocket ID
 * OIDC com discovery automático
 * Endpoints vêm do banco de dados
 */
const pocketidConfig: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid-configuration",
  authMethod: "body",
  fieldMappings: {
    id: ["sub"],
    email: ["email"],
    name: ["name", "preferred_username"],
    firstName: ["given_name"],
    lastName: ["family_name"],
    avatar: ["picture"],
  },
  specialHandling: {
    emailFetchRequired: false,
    responseFormat: "json",
  },
};

/**
 * Template genérico ULTRA-INTELIGENTE para providers customizados
 * Detecta automaticamente padrões comuns e se adapta
 */
const genericProviderTemplate: ProviderConfig = {
  supportsDiscovery: true,
  discoveryEndpoint: "/.well-known/openid_configuration",
  fallbackEndpoints: {
    authorizationEndpoint: "/oauth2/authorize",
    tokenEndpoint: "/oauth2/token",
    userInfoEndpoint: "/oauth2/userinfo",
  },
  authMethod: "body",
  specialHandling: {
    emailEndpoint: "",
    emailFetchRequired: false,
    responseFormat: "json",
  },

  fieldMappings: {
    id: ["sub", "id", "user_id", "uid", "userid", "account_id"],
    email: ["email", "mail", "email_address", "preferred_email", "primary_email"],
    name: ["name", "display_name", "full_name", "username", "login", "first_name last_name", "given_name family_name"],
    firstName: ["given_name", "first_name", "firstname", "first", "name"],
    lastName: ["family_name", "last_name", "lastname", "last", "surname"],
    avatar: ["picture", "avatar", "avatar_url", "profile_picture", "photo", "image", "thumbnail"],
  },
};

/**
 * Configuração completa dos providers
 * Exporta todos os providers oficiais e o template genérico
 */
export const providersConfig: ProvidersConfigFile = {
  officialProviders: {
    google: googleConfig,
    discord: discordConfig,
    github: githubConfig,
    auth0: auth0Config,
    kinde: kindeConfig,
    zitadel: zitadelConfig,
    authentik: authentikConfig,
    frontegg: fronteggConfig,
    pocketid: pocketidConfig,
  },
  genericProviderTemplate,
};

/**
 * Exportações individuais para facilitar importação
 */
export {
  discordConfig,
  googleConfig,
  githubConfig,
  auth0Config,
  kindeConfig,
  zitadelConfig,
  authentikConfig,
  fronteggConfig,
  genericProviderTemplate,
};

export function detectProviderType(issuerUrl: string): string {
  const url = issuerUrl.toLowerCase();

  for (const { pattern, type } of PROVIDER_PATTERNS) {
    if (url.includes(pattern)) {
      return type;
    }
  }

  try {
    return new URL(issuerUrl).hostname.replace("www.", "");
  } catch {
    return "custom";
  }
}

export function getProviderScopes(provider: any): string[] {
  if (provider.scope) {
    return provider.scope.split(" ").filter((s: string) => s.trim());
  }

  const detectedType = detectProviderType(provider.issuerUrl || "");
  return (
    DEFAULT_SCOPES_BY_TYPE[detectedType] || DEFAULT_SCOPES_BY_TYPE[provider.type] || ["openid", "profile", "email"]
  );
}

export function shouldSupportDiscovery(providerType: string): boolean {
  return DISCOVERY_SUPPORTED_PROVIDERS.includes(providerType as any);
}

export function getFallbackEndpoints(providerType: string): any {
  return FALLBACK_ENDPOINTS[providerType] || FALLBACK_ENDPOINTS.oidc;
}
