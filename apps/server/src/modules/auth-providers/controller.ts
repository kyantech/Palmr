import { FastifyReply, FastifyRequest } from "fastify";

import { UpdateAuthProviderSchema } from "./dto";
import { AuthProvidersService } from "./service";
import {
  AuthorizeRequest,
  CallbackRequest,
  CreateProviderRequest,
  DeleteProviderRequest,
  RequestContext,
  UpdateProviderRequest,
  UpdateProvidersOrderRequest,
} from "./types";

const COOKIE_MAX_AGE = 7 * 24 * 60 * 60 * 1000;

const OFFICIAL_PROVIDER_ALLOWED_FIELDS = [
  "issuerUrl",
  "clientId",
  "clientSecret",
  "enabled",
  "autoRegister",
  "adminEmailDomains",
  "icon",
];

const ERROR_MESSAGES = {
  ENDPOINTS_INCOMPLETE:
    "When using manual endpoints, all three endpoints (authorization, token, userInfo) are required",
  MISSING_CONFIG: "Either provide issuerUrl for automatic discovery OR all three custom endpoints",
  PROVIDER_NOT_FOUND: "Provider not found",
  INVALID_URL: "Invalid Provider URL format",
  INVALID_DATA: "Invalid data provided",
  OFFICIAL_CANNOT_DELETE: "Official providers cannot be deleted",
  INVALID_PROVIDERS_ARRAY: "Invalid providers array",
  AUTHORIZATION_FAILED: "Authorization failed",
  AUTHENTICATION_FAILED: "Authentication failed",
} as const;

export class AuthProvidersController {
  private authProvidersService: AuthProvidersService;

  constructor() {
    this.authProvidersService = new AuthProvidersService();
  }

  private buildRequestContext(request: FastifyRequest): RequestContext {
    return {
      protocol: (request.headers["x-forwarded-proto"] as string) || request.protocol,
      host: (request.headers["x-forwarded-host"] as string) || (request.headers.host as string),
      headers: request.headers,
    };
  }

  private buildBaseUrl(requestContext: RequestContext): string {
    return `${requestContext.protocol}://${requestContext.host}`;
  }

  private sendSuccessResponse(reply: FastifyReply, data?: any, message?: string) {
    return reply.send({
      success: true,
      ...(data && { data }),
      ...(message && { message }),
    });
  }

  private sendErrorResponse(reply: FastifyReply, status: number, error: string) {
    return reply.status(status).send({
      success: false,
      error,
    });
  }

  private async handleControllerError(reply: FastifyReply, error: unknown, defaultMessage: string) {
    console.error(`Controller error: ${defaultMessage}`, error);

    if (error instanceof Error && error.message.includes("Either provide issuerUrl")) {
      return this.sendErrorResponse(reply, 400, error.message);
    }

    return this.sendErrorResponse(reply, 500, defaultMessage);
  }

  private validateCustomEndpoints(data: any): string | null {
    const hasAnyCustomEndpoint = !!(data.authorizationEndpoint || data.tokenEndpoint || data.userInfoEndpoint);
    const hasAllCustomEndpoints = !!(data.authorizationEndpoint && data.tokenEndpoint && data.userInfoEndpoint);

    if (hasAnyCustomEndpoint && !hasAllCustomEndpoints) {
      return ERROR_MESSAGES.ENDPOINTS_INCOMPLETE;
    }

    if (!data.issuerUrl && !hasAllCustomEndpoints) {
      return ERROR_MESSAGES.MISSING_CONFIG;
    }

    return null;
  }

  private validateIssuerUrl(issuerUrl: string): boolean {
    try {
      new URL(issuerUrl);
      return true;
    } catch {
      return false;
    }
  }

  private sanitizeOfficialProviderData(data: any): any {
    const sanitizedData: any = {};

    for (const field of OFFICIAL_PROVIDER_ALLOWED_FIELDS) {
      if (data[field] !== undefined) {
        sanitizedData[field] = data[field];
      }
    }

    return sanitizedData;
  }

  private setAuthCookie(reply: FastifyReply, token: string, isSecure: boolean) {
    reply.setCookie("token", token, {
      httpOnly: true,
      secure: isSecure,
      sameSite: "lax",
      maxAge: COOKIE_MAX_AGE,
      path: "/",
    });
  }

  private determineCallbackError(error: Error, provider: string): { type: string; message: string } {
    const errorMessage = error.message;

    if (errorMessage.includes("registration via") && errorMessage.includes("disabled")) {
      return {
        type: "registration_disabled",
        message: `Registration via ${provider} is disabled. Contact your administrator.`,
      };
    }

    if (errorMessage.includes("not enabled")) {
      return {
        type: "provider_disabled",
        message: `${provider} authentication is currently disabled.`,
      };
    }

    if (errorMessage.includes("expired")) {
      return {
        type: "state_expired",
        message: "Authentication session expired. Please try again.",
      };
    }

    if (errorMessage.includes("No email found")) {
      return {
        type: "no_email",
        message: `No email address found in your ${provider} account.`,
      };
    }

    if (errorMessage.includes("Token exchange failed")) {
      return {
        type: "token_exchange_failed",
        message: `Failed to authenticate with ${provider}. Please try again.`,
      };
    }

    if (errorMessage.includes("Missing required user information")) {
      return {
        type: "missing_user_info",
        message: `Incomplete user information from ${provider}.`,
      };
    }

    return {
      type: "unknown_error",
      message: ERROR_MESSAGES.AUTHENTICATION_FAILED,
    };
  }

  async getProviders(request: FastifyRequest, reply: FastifyReply) {
    try {
      const requestContext = this.buildRequestContext(request);
      const providers = await this.authProvidersService.getEnabledProviders(requestContext);

      return this.sendSuccessResponse(reply, providers);
    } catch (error) {
      return this.handleControllerError(reply, error, "Failed to get auth providers");
    }
  }

  async getAllProviders(request: FastifyRequest, reply: FastifyReply) {
    if (reply.sent) return;

    try {
      const providers = await this.authProvidersService.getAllProviders();
      return this.sendSuccessResponse(reply, providers);
    } catch (error) {
      return this.handleControllerError(reply, error, "Failed to get providers");
    }
  }

  async createProvider(request: FastifyRequest<CreateProviderRequest>, reply: FastifyReply) {
    if (reply.sent) return;

    try {
      const data = request.body;

      const validationError = this.validateCustomEndpoints(data);
      if (validationError) {
        return this.sendErrorResponse(reply, 400, validationError);
      }

      const provider = await this.authProvidersService.createProvider(data);
      return this.sendSuccessResponse(reply, provider);
    } catch (error) {
      return this.handleControllerError(reply, error, "Failed to create provider");
    }
  }

  async updateProvider(request: FastifyRequest<UpdateProviderRequest>, reply: FastifyReply) {
    if (reply.sent) return;

    try {
      const { id } = request.params;
      const data = request.body;

      const existingProvider = await this.authProvidersService.getProviderById(id);
      if (!existingProvider) {
        return this.sendErrorResponse(reply, 404, ERROR_MESSAGES.PROVIDER_NOT_FOUND);
      }

      const isOfficial = this.authProvidersService.isOfficialProvider(existingProvider.name);

      if (isOfficial) {
        return this.updateOfficialProvider(reply, id, data);
      }

      return this.updateCustomProvider(reply, id, data);
    } catch (error) {
      return this.handleControllerError(reply, error, "Failed to update provider");
    }
  }

  private async updateOfficialProvider(reply: FastifyReply, id: string, data: any) {
    const sanitizedData = this.sanitizeOfficialProviderData(data);

    if (sanitizedData.issuerUrl && typeof sanitizedData.issuerUrl === "string") {
      if (!this.validateIssuerUrl(sanitizedData.issuerUrl)) {
        return this.sendErrorResponse(reply, 400, ERROR_MESSAGES.INVALID_URL);
      }
    }

    const provider = await this.authProvidersService.updateProvider(id, sanitizedData);
    return this.sendSuccessResponse(reply, provider);
  }

  private async updateCustomProvider(reply: FastifyReply, id: string, data: any) {
    try {
      const validatedData = UpdateAuthProviderSchema.parse(data);
      const provider = await this.authProvidersService.updateProvider(id, validatedData);
      return this.sendSuccessResponse(reply, provider);
    } catch (validationError) {
      console.error("Validation error for custom provider:", validationError);
      console.error("Raw data that failed validation:", data);
      return this.sendErrorResponse(reply, 400, ERROR_MESSAGES.INVALID_DATA);
    }
  }

  async updateProvidersOrder(request: FastifyRequest<UpdateProvidersOrderRequest>, reply: FastifyReply) {
    if (reply.sent) return;

    try {
      const { providers } = request.body;

      if (!Array.isArray(providers)) {
        return this.sendErrorResponse(reply, 400, ERROR_MESSAGES.INVALID_PROVIDERS_ARRAY);
      }

      await this.authProvidersService.updateProvidersOrder(providers);
      return this.sendSuccessResponse(reply, undefined, "Providers order updated successfully");
    } catch (error) {
      return this.handleControllerError(reply, error, "Failed to update providers order");
    }
  }

  async deleteProvider(request: FastifyRequest<DeleteProviderRequest>, reply: FastifyReply) {
    if (reply.sent) return;

    try {
      const { id } = request.params;

      const provider = await this.authProvidersService.getProviderById(id);
      if (!provider) {
        return this.sendErrorResponse(reply, 404, ERROR_MESSAGES.PROVIDER_NOT_FOUND);
      }

      const isOfficial = this.authProvidersService.isOfficialProvider(provider.name);
      if (isOfficial) {
        return this.sendErrorResponse(reply, 400, ERROR_MESSAGES.OFFICIAL_CANNOT_DELETE);
      }

      await this.authProvidersService.deleteProvider(id);
      return this.sendSuccessResponse(reply, undefined, "Provider deleted successfully");
    } catch (error) {
      return this.handleControllerError(reply, error, "Failed to delete provider");
    }
  }

  async authorize(request: FastifyRequest<AuthorizeRequest>, reply: FastifyReply) {
    try {
      const { provider: providerName } = request.params;
      const { state, redirect_uri } = request.query;

      const requestContext = this.buildRequestContext(request);
      const authUrl = await this.authProvidersService.getAuthorizationUrl(
        providerName,
        state,
        redirect_uri,
        requestContext
      );

      return reply.redirect(authUrl);
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : ERROR_MESSAGES.AUTHORIZATION_FAILED;
      return this.sendErrorResponse(reply, 400, errorMessage);
    }
  }

  async callback(request: FastifyRequest<CallbackRequest>, reply: FastifyReply) {
    try {
      const { provider: providerName } = request.params;
      const { code, state, error } = request.query;

      const requestContext = this.buildRequestContext(request);
      const baseUrl = this.buildBaseUrl(requestContext);

      if (error) {
        return reply.redirect(`${baseUrl}/login?error=oauth_error&provider=${providerName}`);
      }

      if (!code) {
        return reply.redirect(`${baseUrl}/login?error=missing_code&provider=${providerName}`);
      }

      if (!state) {
        return reply.redirect(`${baseUrl}/login?error=missing_parameters&provider=${providerName}`);
      }

      const result = await this.authProvidersService.handleCallback(providerName, code, state, requestContext);

      const jwt = await request.jwtSign({
        userId: result.user.id,
        isAdmin: result.user.isAdmin,
      });

      this.setAuthCookie(reply, jwt, request.protocol === "https");

      const redirectUrl = result.redirectUrl || "/dashboard";
      const fullRedirectUrl = redirectUrl.startsWith("http") ? redirectUrl : `${baseUrl}${redirectUrl}`;

      return reply.redirect(fullRedirectUrl);
    } catch (error) {
      return this.handleCallbackError(request, reply, error);
    }
  }

  private handleCallbackError(request: FastifyRequest<CallbackRequest>, reply: FastifyReply, error: unknown) {
    // Log error for debugging
    console.error("Auth callback error for provider:", request.params.provider, error);

    const { type: errorType, message: errorMessage } =
      error instanceof Error
        ? this.determineCallbackError(error, request.params.provider)
        : { type: "unknown_error", message: ERROR_MESSAGES.AUTHENTICATION_FAILED };

    const requestContext = this.buildRequestContext(request);
    const baseUrl = this.buildBaseUrl(requestContext);
    const encodedMessage = encodeURIComponent(errorMessage);

    return reply.redirect(
      `${baseUrl}/login?error=${errorType}&provider=${request.params.provider}&message=${encodedMessage}`
    );
  }
}
