import type { TFunction } from "i18next";
import { z } from "zod";
import type {
  CreateProviderRequest,
  DiscoveredProvider,
  PresetKey,
  Provider,
  ProviderPreset,
  ProviderProtocol,
  TokenAuthMethod,
  UpdateProviderRequest,
} from "../types";
import { TOKEN_AUTH_METHODS } from "../types";

const PRESET_KEYS = [
  "google",
  "github",
  "discord",
  "auth0",
  "kinde",
  "zitadel",
  "authentik",
  "frontegg",
  "pocket_id",
  "generic",
] as const satisfies readonly PresetKey[];

export const SLUG_PATTERN = /^[a-z0-9_-]{2,40}$/;

export interface ProviderFormValues {
  presetKey: string;
  slug: string;
  displayName: string;
  protocol: ProviderProtocol;
  preset: PresetKey;
  issuerUrl: string;
  clientId: string;
  clientSecret: string;
  clearSecret: boolean;
  tokenAuthMethod: TokenAuthMethod;
  scopes: string[];
  authorizationEndpoint: string;
  tokenEndpoint: string;
  userinfoEndpoint: string;
  jwksEndpoint: string;
  claimSubject: string;
  claimEmail: string;
  claimEmailVerified: string;
  claimUsername: string;
  claimName: string;
  claimPicture: string;
  autoProvision: boolean;
  allowEmailLinking: boolean;
  enabled: boolean;
}

export const FORM_FIELD_NAMES = [
  "slug",
  "displayName",
  "issuerUrl",
  "clientId",
  "clientSecret",
  "tokenAuthMethod",
  "scopes",
  "endpoints",
  "claimMapping",
] as const;

export function presetKeyOf(preset: Pick<ProviderPreset, "preset" | "protocol">): string {
  return `${preset.preset}:${preset.protocol}`;
}

export function emptyValues(): ProviderFormValues {
  return {
    presetKey: "",
    slug: "",
    displayName: "",
    protocol: "oidc",
    preset: "generic",
    issuerUrl: "",
    clientId: "",
    clientSecret: "",
    clearSecret: false,
    tokenAuthMethod: "client_secret_basic",
    scopes: ["openid", "email", "profile"],
    authorizationEndpoint: "",
    tokenEndpoint: "",
    userinfoEndpoint: "",
    jwksEndpoint: "",
    claimSubject: "sub",
    claimEmail: "email",
    claimEmailVerified: "email_verified",
    claimUsername: "preferred_username",
    claimName: "name",
    claimPicture: "picture",
    autoProvision: false,
    allowEmailLinking: true,
    enabled: false,
  };
}

export function valuesFromPreset(preset: ProviderPreset): ProviderFormValues {
  return {
    ...emptyValues(),
    presetKey: presetKeyOf(preset),
    slug: preset.preset === "generic" ? "" : preset.preset,
    displayName: preset.displayName,
    protocol: preset.protocol,
    preset: preset.preset,
    issuerUrl: preset.issuerUrl ?? "",
    tokenAuthMethod: preset.tokenAuthMethod,
    scopes: [...preset.scopes],
    authorizationEndpoint: preset.endpoints.authorization ?? "",
    tokenEndpoint: preset.endpoints.token ?? "",
    userinfoEndpoint: preset.endpoints.userinfo ?? "",
    jwksEndpoint: preset.endpoints.jwks ?? "",
    claimSubject: preset.claimMapping.subject,
    claimEmail: preset.claimMapping.email,
    claimEmailVerified: preset.claimMapping.emailVerified,
    claimUsername: preset.claimMapping.username,
    claimName: preset.claimMapping.name,
    claimPicture: preset.claimMapping.picture,
    allowEmailLinking: preset.allowEmailLinking,
  };
}

export function valuesFromProvider(provider: Provider): ProviderFormValues {
  return {
    presetKey: `${provider.preset}:${provider.protocol}`,
    slug: provider.slug,
    displayName: provider.displayName,
    protocol: provider.protocol,
    preset: provider.preset,
    issuerUrl: provider.issuerUrl ?? "",
    clientId: provider.clientId,
    clientSecret: "",
    clearSecret: false,
    tokenAuthMethod: provider.tokenAuthMethod,
    scopes: [...provider.scopes],
    authorizationEndpoint: provider.endpoints.authorization ?? "",
    tokenEndpoint: provider.endpoints.token ?? "",
    userinfoEndpoint: provider.endpoints.userinfo ?? "",
    jwksEndpoint: provider.endpoints.jwks ?? "",
    claimSubject: provider.claimMapping.subject,
    claimEmail: provider.claimMapping.email,
    claimEmailVerified: provider.claimMapping.emailVerified,
    claimUsername: provider.claimMapping.username,
    claimName: provider.claimMapping.name,
    claimPicture: provider.claimMapping.picture,
    autoProvision: provider.autoProvision,
    allowEmailLinking: provider.allowEmailLinking,
    enabled: provider.enabled,
  };
}

export function applyDiscovery(
  values: ProviderFormValues,
  discovered: DiscoveredProvider,
): ProviderFormValues {
  return {
    ...values,
    issuerUrl: discovered.issuerUrl,
    authorizationEndpoint: discovered.endpoints.authorization ?? values.authorizationEndpoint,
    tokenEndpoint: discovered.endpoints.token ?? values.tokenEndpoint,
    userinfoEndpoint: discovered.endpoints.userinfo ?? values.userinfoEndpoint,
    jwksEndpoint: discovered.endpoints.jwks ?? values.jwksEndpoint,
  };
}

function isHttpUrl(value: string): boolean {
  try {
    const { protocol } = new URL(value);
    return protocol === "https:" || protocol === "http:";
  } catch {
    return false;
  }
}

export type FormMode = "create" | "edit";

export function providerSchema(t: TFunction<"admin">, mode: FormMode) {
  const required = t("validation.required");
  const url = t("providers.form.validation.url");
  const optionalUrl = z
    .string()
    .trim()
    .refine((value) => value === "" || isHttpUrl(value), url);
  const claim = z.string().trim().min(1, required);
  return z
    .object({
      presetKey: z.string(),
      slug: z.string().trim(),
      displayName: z.string().trim().min(1, required),
      protocol: z.enum(["oidc", "oauth2"]),
      preset: z.enum(PRESET_KEYS),
      issuerUrl: optionalUrl,
      clientId: z.string().trim().min(1, required),
      clientSecret: z.string(),
      clearSecret: z.boolean(),
      tokenAuthMethod: z.enum(TOKEN_AUTH_METHODS),
      scopes: z.array(z.string().trim().min(1)),
      authorizationEndpoint: optionalUrl,
      tokenEndpoint: optionalUrl,
      userinfoEndpoint: optionalUrl,
      jwksEndpoint: optionalUrl,
      claimSubject: claim,
      claimEmail: claim,
      claimEmailVerified: claim,
      claimUsername: claim,
      claimName: claim,
      claimPicture: claim,
      autoProvision: z.boolean(),
      allowEmailLinking: z.boolean(),
      enabled: z.boolean(),
    })
    .superRefine((values, context) => {
      const need = (path: keyof ProviderFormValues, message: string) => {
        context.addIssue({ code: "custom", path: [path], message });
      };
      if (mode === "create") {
        if (values.presetKey === "") {
          need("presetKey", required);
        }
        if (!SLUG_PATTERN.test(values.slug)) {
          need("slug", t("providers.form.validation.slug"));
        }
        if (values.tokenAuthMethod !== "none" && values.clientSecret === "") {
          need("clientSecret", required);
        }
      }
      if (values.protocol === "oidc") {
        if (values.issuerUrl === "") {
          need("issuerUrl", required);
        }
        if (!values.scopes.includes("openid")) {
          need("scopes", t("providers.form.validation.openid"));
        }
      } else {
        if (values.authorizationEndpoint === "") {
          need("authorizationEndpoint", required);
        }
        if (values.tokenEndpoint === "") {
          need("tokenEndpoint", required);
        }
        if (values.userinfoEndpoint === "") {
          need("userinfoEndpoint", required);
        }
      }
    });
}

function endpointsOf(values: ProviderFormValues) {
  const entries: [string, string][] = [
    ["authorization", values.authorizationEndpoint.trim()],
    ["token", values.tokenEndpoint.trim()],
    ["userinfo", values.userinfoEndpoint.trim()],
    ["jwks", values.jwksEndpoint.trim()],
  ];
  return Object.fromEntries(entries.filter(([, value]) => value !== ""));
}

export function createBody(values: ProviderFormValues): CreateProviderRequest {
  const endpoints = endpointsOf(values);
  return {
    slug: values.slug.trim(),
    displayName: values.displayName.trim(),
    protocol: values.protocol,
    preset: values.preset,
    ...(values.protocol === "oidc" ? { issuerUrl: values.issuerUrl.trim() } : {}),
    clientId: values.clientId.trim(),
    ...(values.clientSecret === "" ? {} : { clientSecret: values.clientSecret }),
    tokenAuthMethod: values.tokenAuthMethod,
    scopes: values.scopes.map((scope) => scope.trim()),
    ...(Object.keys(endpoints).length === 0 ? {} : { endpoints }),
    claimMapping: claimMappingOf(values),
    autoProvision: values.autoProvision,
    allowEmailLinking: values.allowEmailLinking,
    enabled: values.enabled,
  };
}

function claimMappingOf(values: ProviderFormValues) {
  return {
    subject: values.claimSubject.trim(),
    email: values.claimEmail.trim(),
    emailVerified: values.claimEmailVerified.trim(),
    username: values.claimUsername.trim(),
    name: values.claimName.trim(),
    picture: values.claimPicture.trim(),
  };
}

function sameList(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((item, index) => item === right[index]);
}

export function updateBody(provider: Provider, values: ProviderFormValues): UpdateProviderRequest {
  const body: UpdateProviderRequest = {};
  if (values.displayName.trim() !== provider.displayName) {
    body.displayName = values.displayName.trim();
  }
  if (provider.protocol === "oidc" && values.issuerUrl.trim() !== (provider.issuerUrl ?? "")) {
    body.issuerUrl = values.issuerUrl.trim();
  }
  if (values.clientId.trim() !== provider.clientId) {
    body.clientId = values.clientId.trim();
  }
  if (values.clientSecret !== "") {
    body.clientSecret = values.clientSecret;
  } else if (
    values.clearSecret &&
    values.tokenAuthMethod === "none" &&
    provider.clientSecretConfigured
  ) {
    body.clientSecret = null;
  }
  if (values.tokenAuthMethod !== provider.tokenAuthMethod) {
    body.tokenAuthMethod = values.tokenAuthMethod;
  }
  if (!sameList(values.scopes, provider.scopes)) {
    body.scopes = values.scopes.map((scope) => scope.trim());
  }
  const endpoints: Record<string, string | null> = {};
  const members: [keyof Provider["endpoints"], string][] = [
    ["authorization", values.authorizationEndpoint.trim()],
    ["token", values.tokenEndpoint.trim()],
    ["userinfo", values.userinfoEndpoint.trim()],
    ["jwks", values.jwksEndpoint.trim()],
  ];
  for (const [member, value] of members) {
    const stored = provider.endpoints[member] ?? "";
    if (value !== stored) {
      endpoints[member] = value === "" ? null : value;
    }
  }
  if (Object.keys(endpoints).length > 0) {
    body.endpoints = endpoints;
  }
  const claims = claimMappingOf(values);
  const stored = provider.claimMapping;
  const claimChanges = Object.fromEntries(
    Object.entries(claims).filter(([key, value]) => value !== stored[key as keyof typeof stored]),
  );
  if (Object.keys(claimChanges).length > 0) {
    body.claimMapping = claimChanges;
  }
  if (values.autoProvision !== provider.autoProvision) {
    body.autoProvision = values.autoProvision;
  }
  if (values.allowEmailLinking !== provider.allowEmailLinking) {
    body.allowEmailLinking = values.allowEmailLinking;
  }
  return body;
}

export interface ParsedFailure {
  readonly name: string;
  readonly reason: string;
}

export function parseValidationError(value: string | null): ParsedFailure[] {
  if (value === null || value === "") {
    return [];
  }
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry !== "")
    .map((entry) => {
      const separator = entry.indexOf(":");
      return separator === -1
        ? { name: entry, reason: "failed" }
        : { name: entry.slice(0, separator), reason: entry.slice(separator + 1) };
    });
}
