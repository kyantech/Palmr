import crypto from "crypto";

import { prisma } from "../../shared/prisma";
import {
  detectProviderType,
  DISCOVERY_PATHS,
  getFallbackEndpoints,
  getProviderScopes,
  providersConfig,
  shouldSupportDiscovery,
} from "./providers.config";
import {
  PendingState,
  ProviderConfig,
  ProviderEndpoints,
  ProviderUserInfo,
  RequestContextService,
  TokenResponse,
} from "./types";

// Constants
const DEFAULT_BASE_URL = "http://localhost:3000";
const STATE_EXPIRY_TIME = 600000; // 10 minutes
const CLEANUP_INTERVAL = 5 * 60 * 1000; // 5 minutes
const DEFAULT_PROVIDER_TYPE = "oidc";

const ERROR_MESSAGES = {
  PROVIDER_NOT_FOUND: "Provider not found or disabled",
  CONFIG_NOT_FOUND: "Configuration not found for provider",
  INVALID_STATE: "Invalid or expired state",
  CLIENT_ID_MISSING: "Client ID not configured for provider",
  TOKEN_EXCHANGE_FAILED: "Token exchange failed",
  NO_ACCESS_TOKEN: "No access token received",
  USERINFO_FAILED: "UserInfo request failed",
  NO_EMAIL_FOUND: "No email address found in account",
  MISSING_USER_INFO: "Missing required user information (email or external ID)",
} as const;

export class AuthProvidersService {
  private pendingStates = new Map<string, PendingState>();

  constructor() {
    setInterval(() => this.cleanupExpiredStates(), CLEANUP_INTERVAL);
  }

  // Utility methods
  private buildBaseUrl(requestContext?: RequestContextService): string {
    return requestContext ? `${requestContext.protocol}://${requestContext.host}` : DEFAULT_BASE_URL;
  }

  private generateState(): string {
    return crypto.randomBytes(32).toString("hex");
  }

  private generateCodeVerifier(): string {
    return crypto.randomBytes(32).toString("base64url");
  }

  private generateCodeChallenge(codeVerifier: string): string {
    return crypto.createHash("sha256").update(codeVerifier).digest("base64url");
  }

  private createPendingState(providerId: string, codeVerifier: string, redirectUrl: string): PendingState {
    return {
      codeVerifier,
      redirectUrl,
      expiresAt: Date.now() + STATE_EXPIRY_TIME,
      providerId,
    };
  }

  private validateProvider(provider: any, providerName: string): void {
    if (!provider || !provider.enabled) {
      throw new Error(`${ERROR_MESSAGES.PROVIDER_NOT_FOUND}: ${providerName}`);
    }
  }

  private validateConfig(config: ProviderConfig | null, providerName: string): void {
    if (!config) {
      throw new Error(`${ERROR_MESSAGES.CONFIG_NOT_FOUND}: ${providerName}`);
    }
  }

  private validateClientId(provider: any, providerName: string): void {
    if (!provider.clientId) {
      throw new Error(`${ERROR_MESSAGES.CLIENT_ID_MISSING}: ${providerName}`);
    }
  }

  // Provider configuration methods
  private isOfficial(providerName: string): boolean {
    return providerName in providersConfig.officialProviders;
  }

  private getProviderConfig(provider: any): ProviderConfig {
    const officialConfig = providersConfig.officialProviders[provider.name];
    if (officialConfig) {
      return officialConfig;
    }

    const detectedType = detectProviderType(provider.issuerUrl || "");
    const providerType = provider.type || detectedType;

    return {
      ...providersConfig.genericProviderTemplate,
      name: provider.name,
      supportsDiscovery: shouldSupportDiscovery(providerType),
      discoveryEndpoint: "/.well-known/openid_configuration",
      fallbackEndpoints: getFallbackEndpoints(providerType),
      authMethod: "body",
      fieldMappings: providersConfig.genericProviderTemplate.fieldMappings,
      specialHandling: providersConfig.genericProviderTemplate.specialHandling,
    };
  }

  private async resolveEndpoints(provider: any, config: ProviderConfig): Promise<ProviderEndpoints> {
    // Use custom endpoints if all are provided
    if (provider.authorizationEndpoint && provider.tokenEndpoint && provider.userInfoEndpoint) {
      return {
        authorizationEndpoint: this.resolveEndpointUrl(provider.authorizationEndpoint, provider.issuerUrl),
        tokenEndpoint: this.resolveEndpointUrl(provider.tokenEndpoint, provider.issuerUrl),
        userInfoEndpoint: this.resolveEndpointUrl(provider.userInfoEndpoint, provider.issuerUrl),
      };
    }

    // Try discovery if supported
    if (config.supportsDiscovery && provider.issuerUrl) {
      const discoveredEndpoints = await this.attemptDiscovery(provider.issuerUrl);
      if (discoveredEndpoints) {
        return discoveredEndpoints;
      }
    }

    // Fallback to intelligent endpoints
    const baseUrl = provider.issuerUrl?.replace(/\/$/, "") || "";
    const detectedType = detectProviderType(provider.issuerUrl || "");
    const fallbackPattern = getFallbackEndpoints(detectedType);

    return {
      authorizationEndpoint: `${baseUrl}${fallbackPattern.authorizationEndpoint}`,
      tokenEndpoint: `${baseUrl}${fallbackPattern.tokenEndpoint}`,
      userInfoEndpoint: `${baseUrl}${fallbackPattern.userInfoEndpoint}`,
    };
  }

  private resolveEndpointUrl(endpoint: string, issuerUrl?: string): string {
    if (endpoint.startsWith("http://") || endpoint.startsWith("https://")) {
      return endpoint;
    }

    if (!issuerUrl) {
      return endpoint;
    }

    const baseUrl = issuerUrl.replace(/\/$/, "");
    const path = endpoint.startsWith("/") ? endpoint : `/${endpoint}`;
    return `${baseUrl}${path}`;
  }

  private async attemptDiscovery(issuerUrl: string): Promise<ProviderEndpoints | null> {
    for (const discoveryPath of DISCOVERY_PATHS) {
      try {
        const discoveryUrl = `${issuerUrl}${discoveryPath}`;
        const response = await fetch(discoveryUrl, {
          method: "GET",
          headers: { Accept: "application/json" },
          signal: AbortSignal.timeout(10000),
        });

        if (!response.ok) continue;

        const discoveryData = (await response.json()) as any;
        const endpoints = {
          authorizationEndpoint: discoveryData.authorization_endpoint || "",
          tokenEndpoint: discoveryData.token_endpoint || "",
          userInfoEndpoint: discoveryData.userinfo_endpoint || "",
        };

        if (endpoints.authorizationEndpoint && endpoints.tokenEndpoint) {
          return endpoints;
        }
      } catch {
        continue;
      }
    }
    return null;
  }

  private getAuthMethod(config: ProviderConfig): "body" | "basic" | "header" {
    return config.authMethod || "body";
  }

  private extractUserInfo(rawUserInfo: any, config: ProviderConfig): ProviderUserInfo {
    const userInfo: ProviderUserInfo = { id: "", email: "" };
    const mappings = config.fieldMappings;

    userInfo.id = this.extractField(rawUserInfo, mappings.id) || "";
    userInfo.email = this.extractField(rawUserInfo, mappings.email) || "";
    userInfo.name = this.extractField(rawUserInfo, mappings.name);
    userInfo.firstName = this.extractField(rawUserInfo, mappings.firstName);
    userInfo.lastName = this.extractField(rawUserInfo, mappings.lastName);
    userInfo.avatar = this.extractField(rawUserInfo, mappings.avatar);

    return userInfo;
  }

  private extractField(obj: any, fieldNames: string[]): string | undefined {
    if (!obj || !fieldNames.length) return undefined;

    for (const fieldName of fieldNames) {
      if (obj[fieldName] !== undefined && obj[fieldName] !== null) {
        return String(obj[fieldName]);
      }
    }

    return undefined;
  }

  private requiresEmailFetch(config: ProviderConfig): boolean {
    return config.specialHandling?.emailFetchRequired || false;
  }

  private getEmailEndpoint(config: ProviderConfig): string | null {
    return config.specialHandling?.emailEndpoint || null;
  }

  // PKCE and OAuth setup methods
  private setupPkceIfNeeded(provider: any): { codeVerifier?: string; codeChallenge?: string } {
    const needsPkce = provider.type === DEFAULT_PROVIDER_TYPE;

    if (needsPkce) {
      const codeVerifier = this.generateCodeVerifier();
      const codeChallenge = this.generateCodeChallenge(codeVerifier);
      return { codeVerifier, codeChallenge };
    }

    return {};
  }

  private async buildAuthorizationUrl(
    provider: any,
    endpoints: any,
    callbackUrl: string,
    state: string,
    codeChallenge?: string,
    providerName?: string
  ): Promise<string> {
    this.validateClientId(provider, providerName || provider.name);

    const authUrl = new URL(endpoints.authorizationEndpoint);
    const scopes = getProviderScopes(provider);
    authUrl.searchParams.set("client_id", provider.clientId);
    authUrl.searchParams.set("redirect_uri", callbackUrl);
    authUrl.searchParams.set("response_type", "code");
    authUrl.searchParams.set("scope", scopes.join(" "));
    authUrl.searchParams.set("state", state);

    if (codeChallenge) {
      authUrl.searchParams.set("code_challenge", codeChallenge);
      authUrl.searchParams.set("code_challenge_method", "S256");
    }

    return authUrl.toString();
  }

  // Callback handling methods
  private validateAndGetPendingState(state: string): PendingState {
    const pendingState = this.pendingStates.get(state);

    if (!pendingState) {
      throw new Error(ERROR_MESSAGES.INVALID_STATE);
    }

    return pendingState;
  }

  private async executeAuthenticationFlow(
    provider: any,
    config: ProviderConfig,
    code: string,
    pendingState: PendingState,
    requestContext?: RequestContextService
  ) {
    const authResult = await this.performTokenExchange(
      provider,
      config,
      code,
      pendingState.codeVerifier,
      requestContext
    );

    const userInfo = await this.processUserInfo(authResult.userInfo, authResult.tokens, config);
    const user = await this.findOrCreateUser(userInfo, provider);

    return {
      user,
      isNewUser: false,
      redirectUrl: pendingState.redirectUrl,
    };
  }

  // Public methods
  async getEnabledProviders(requestContext?: RequestContextService) {
    const providers = await prisma.authProvider.findMany({
      where: { enabled: true },
      orderBy: { sortOrder: "asc" },
      select: {
        id: true,
        name: true,
        displayName: true,
        type: true,
        enabled: true,
        issuerUrl: true,
        icon: true,
        sortOrder: true,
      },
    });

    return providers.map((provider) => {
      const authUrl = this.generateAuthUrl(provider, requestContext);

      return {
        id: provider.id,
        name: provider.name,
        displayName: provider.displayName || provider.name,
        type: provider.type,
        icon: provider.icon || "generic",
        authUrl,
        isOfficial: this.isOfficial(provider.name),
        sortOrder: provider.sortOrder,
      };
    });
  }

  async getAllProviders() {
    const providers = await prisma.authProvider.findMany({
      orderBy: { sortOrder: "asc" },
    });

    return providers.map((provider) => ({
      ...provider,
      isOfficial: this.isOfficial(provider.name),
    }));
  }

  async getProviderByName(name: string) {
    return await prisma.authProvider.findFirst({
      where: { name },
    });
  }

  async getProviderById(id: string) {
    return await prisma.authProvider.findUnique({
      where: { id },
    });
  }

  isOfficialProvider(providerName: string): boolean {
    return this.isOfficial(providerName);
  }

  async createProvider(data: any) {
    return await prisma.authProvider.create({
      data: {
        ...data,
        type: data.type || DEFAULT_PROVIDER_TYPE,
        displayName: data.displayName || data.name,
      },
    });
  }

  async updateProvider(id: string, data: any) {
    return await prisma.authProvider.update({
      where: { id },
      data,
    });
  }

  async deleteProvider(id: string) {
    return await prisma.authProvider.delete({
      where: { id },
    });
  }

  private generateAuthUrl(provider: any, requestContext?: RequestContextService) {
    const baseUrl = this.buildBaseUrl(requestContext);
    return `${baseUrl}/api/auth/providers/${provider.name}/authorize`;
  }

  async getAuthorizationUrl(
    providerName: string,
    state?: string,
    redirectUri?: string,
    requestContext?: RequestContextService
  ) {
    const provider = await this.getProviderByName(providerName);
    this.validateProvider(provider, providerName);
    const validatedProvider = provider!;

    const config = this.getProviderConfig(validatedProvider);
    this.validateConfig(config, providerName);
    const validatedConfig = config!;

    const finalState = state || this.generateState();
    const baseUrl = this.buildBaseUrl(requestContext);
    const callbackUrl = redirectUri || `${baseUrl}/api/auth/providers/${providerName}/callback`;

    const { codeVerifier, codeChallenge } = this.setupPkceIfNeeded(validatedProvider);

    const pendingState = this.createPendingState(
      validatedProvider.id,
      codeVerifier || "",
      redirectUri || `${baseUrl}/dashboard`
    );
    this.pendingStates.set(finalState, pendingState);

    const endpoints = await this.resolveEndpoints(validatedProvider, validatedConfig);

    const finalAuthUrl = await this.buildAuthorizationUrl(
      validatedProvider,
      endpoints,
      callbackUrl,
      finalState,
      codeChallenge,
      providerName
    );

    return finalAuthUrl;
  }

  async handleCallback(providerName: string, code: string, state: string, requestContext?: RequestContextService) {
    try {
      const pendingState = this.validateAndGetPendingState(state);

      const provider = await this.getProviderByName(providerName);
      this.validateProvider(provider, providerName);
      const validatedProvider = provider!;

      const config = this.getProviderConfig(validatedProvider);
      this.validateConfig(config, providerName);
      const validatedConfig = config!;

      return await this.executeAuthenticationFlow(
        validatedProvider,
        validatedConfig,
        code,
        pendingState,
        requestContext
      );
    } catch (error) {
      console.error("Error in handleCallback:", error);
      throw error;
    }
  }

  private async performTokenExchange(
    provider: any,
    config: ProviderConfig,
    code: string,
    codeVerifier: string,
    requestContext?: RequestContextService
  ) {
    const endpoints = await this.resolveEndpoints(provider, config);
    const authMethod = this.getAuthMethod(config);

    const baseUrl = this.buildBaseUrl(requestContext);
    const callbackUrl = provider.redirectUri || `${baseUrl}/api/auth/providers/${provider.name}/callback`;

    const tokens = await this.executeTokenRequest(provider, code, callbackUrl, codeVerifier, authMethod, endpoints);
    const rawUserInfo = await this.fetchUserInfo(tokens, endpoints);

    return {
      userInfo: rawUserInfo,
      tokens,
    };
  }

  private async executeTokenRequest(
    provider: any,
    code: string,
    callbackUrl: string,
    codeVerifier: string,
    authMethod: string,
    endpoints: any
  ): Promise<TokenResponse> {
    const body = new URLSearchParams();
    body.append("client_id", provider.clientId);
    body.append("code", code);
    body.append("redirect_uri", callbackUrl);
    body.append("grant_type", "authorization_code");

    if (authMethod === "body" && provider.clientSecret) {
      body.append("client_secret", provider.clientSecret);
    }

    if (codeVerifier) {
      body.append("code_verifier", codeVerifier);
    }

    const headers: Record<string, string> = {
      "Content-Type": "application/x-www-form-urlencoded",
      Accept: "application/json",
    };

    if (authMethod === "basic" && provider.clientSecret) {
      const auth = Buffer.from(`${provider.clientId}:${provider.clientSecret}`).toString("base64");
      headers["Authorization"] = `Basic ${auth}`;
    }

    const tokenResponse = await fetch(endpoints.tokenEndpoint, {
      method: "POST",
      headers,
      body,
    });

    if (!tokenResponse.ok) {
      const errorText = await tokenResponse.text();
      throw new Error(`${ERROR_MESSAGES.TOKEN_EXCHANGE_FAILED}: ${tokenResponse.status} - ${errorText}`);
    }

    const tokens = (await tokenResponse.json()) as TokenResponse;

    if (!tokens.access_token) {
      throw new Error(ERROR_MESSAGES.NO_ACCESS_TOKEN);
    }

    return tokens;
  }

  private async fetchUserInfo(tokens: TokenResponse, endpoints: any): Promise<any> {
    const userInfoResponse = await fetch(endpoints.userInfoEndpoint, {
      headers: {
        Authorization: `Bearer ${tokens.access_token}`,
        Accept: "application/json",
      },
    });

    if (!userInfoResponse.ok) {
      const errorText = await userInfoResponse.text();
      throw new Error(`${ERROR_MESSAGES.USERINFO_FAILED}: ${userInfoResponse.status} - ${errorText}`);
    }

    return await userInfoResponse.json();
  }

  private async processUserInfo(
    rawUserInfo: any,
    tokens: TokenResponse,
    config: ProviderConfig
  ): Promise<ProviderUserInfo> {
    const userInfo = this.extractUserInfo(rawUserInfo, config);

    if (!userInfo.email && this.requiresEmailFetch(config)) {
      const emailEndpoint = this.getEmailEndpoint(config);
      if (emailEndpoint) {
        const email = await this.fetchEmailFromEndpoint(emailEndpoint, tokens.access_token);
        if (email) {
          userInfo.email = email;
        }
      }
    }

    if (!userInfo.email) {
      throw new Error(`${ERROR_MESSAGES.NO_EMAIL_FOUND} in ${config.name} account`);
    }

    return userInfo;
  }

  private async fetchEmailFromEndpoint(endpoint: string, accessToken: string): Promise<string | null> {
    try {
      const response = await fetch(endpoint, {
        headers: {
          Authorization: `Bearer ${accessToken}`,
          Accept: "application/json",
        },
      });

      if (response.ok) {
        const data: any = await response.json();

        if (Array.isArray(data)) {
          const primaryEmail = data.find((e: any) => e.primary && e.verified);
          if (primaryEmail) return primaryEmail.email;

          const verifiedEmail = data.find((e: any) => e.verified);
          if (verifiedEmail) return verifiedEmail.email;

          if (data.length > 0 && data[0].email) return data[0].email;
        }

        if (data.email) return data.email;
      }
    } catch (error) {
      console.error("Error fetching email from endpoint:", error);
    }

    return null;
  }

  private async findOrCreateUser(userInfo: ProviderUserInfo, provider: any) {
    const externalId = userInfo.id;

    if (!userInfo.email || !externalId) {
      throw new Error(ERROR_MESSAGES.MISSING_USER_INFO);
    }

    // First, check if there's already an auth provider entry for this external ID
    const existingAuthProvider = await this.findExistingAuthProvider(provider.id, String(externalId));
    if (existingAuthProvider) {
      return await this.updateExistingUserFromProvider(existingAuthProvider.user, userInfo);
    }

    // Check if there's a user with this email
    const existingUser = await this.findExistingUserByEmail(userInfo.email);
    if (existingUser) {
      // Check if this user already has this provider linked
      const existingUserProvider = await prisma.userAuthProvider.findFirst({
        where: {
          userId: existingUser.id,
          providerId: provider.id,
        },
      });

      if (existingUserProvider) {
        return await this.updateExistingUserFromProvider(existingUser, userInfo);
      }

      return await this.linkProviderToExistingUser(existingUser, provider.id, String(externalId), userInfo);
    }

    return await this.createNewUserWithProvider(userInfo, provider.id, String(externalId));
  }

  private async findExistingAuthProvider(providerId: string, externalId: string) {
    return await prisma.userAuthProvider.findUnique({
      where: {
        providerId_externalId: {
          providerId,
          externalId,
        },
      },
      include: {
        user: true,
      },
    });
  }

  private async findExistingUserByEmail(email: string) {
    return await prisma.user.findUnique({
      where: { email },
    });
  }

  private buildUserUpdateData(existingUser: any, userInfo: ProviderUserInfo): any {
    const updateData: any = {};

    if (!existingUser.firstName && userInfo.firstName) {
      updateData.firstName = userInfo.firstName;
    }

    if (!existingUser.lastName && userInfo.lastName) {
      updateData.lastName = userInfo.lastName;
    }

    if (!existingUser.image && userInfo.avatar) {
      updateData.image = userInfo.avatar;
    }

    return updateData;
  }

  private async updateExistingUserFromProvider(existingUser: any, userInfo: ProviderUserInfo) {
    const updateData = this.buildUserUpdateData(existingUser, userInfo);

    if (Object.keys(updateData).length > 0) {
      return await prisma.user.update({
        where: { id: existingUser.id },
        data: updateData,
      });
    }

    return existingUser;
  }

  private async linkProviderToExistingUser(
    existingUser: any,
    providerId: string,
    externalId: string,
    userInfo: ProviderUserInfo
  ) {
    await prisma.userAuthProvider.create({
      data: {
        userId: existingUser.id,
        providerId,
        externalId,
      },
    });

    return await this.updateExistingUserFromProvider(existingUser, userInfo);
  }

  private generateUserNames(userInfo: ProviderUserInfo) {
    const displayName = userInfo.name || userInfo.email.split("@")[0];
    const firstName = userInfo.firstName || displayName.split(" ")[0] || userInfo.email.split("@")[0];
    const lastName =
      userInfo.lastName || (displayName.split(" ").length > 1 ? displayName.split(" ").slice(1).join(" ") : "");

    return { firstName, lastName };
  }

  private async createNewUserWithProvider(userInfo: ProviderUserInfo, providerId: string, externalId: string) {
    const { firstName, lastName } = this.generateUserNames(userInfo);

    return await prisma.user.create({
      data: {
        email: userInfo.email,
        username: userInfo.email.split("@")[0],
        firstName,
        lastName,
        image: userInfo.avatar || null,
        isAdmin: false,
        authProviders: {
          create: {
            providerId,
            externalId,
          },
        },
      },
    });
  }

  private cleanupExpiredStates() {
    const now = Date.now();
    for (const [state, data] of this.pendingStates.entries()) {
      if (data.expiresAt < now) {
        this.pendingStates.delete(state);
      }
    }
  }

  async updateProvidersOrder(providersOrder: { id: string; sortOrder: number }[]) {
    const updatePromises = providersOrder.map((provider) =>
      prisma.authProvider.update({
        where: { id: provider.id },
        data: { sortOrder: provider.sortOrder },
      })
    );

    await prisma.$transaction(updatePromises);
    return { success: true };
  }
}
