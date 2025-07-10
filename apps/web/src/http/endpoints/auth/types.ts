import type { AxiosResponse } from "axios";

// Base types that are reused across different operations
export interface BaseUser {
  id: string;
  firstName: string;
  lastName: string;
  username: string;
  email: string;
  isAdmin: boolean;
  isActive: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface User extends BaseUser {
  image: string | null;
}

export type LoginUser = BaseUser;

// Common API response patterns
export interface ApiResponse<T> {
  success: boolean;
  data: T;
}

export interface ApiMessageResponse {
  success: boolean;
  message: string;
}

export interface SimpleMessageResponse {
  message: string;
}

// Auth response types using base types
export interface GetCurrentUser200 {
  user: User;
}

export interface Login200 {
  user: LoginUser;
}

export interface OidcConfig200 {
  enabled: boolean;
  issuer?: string;
  authUrl?: string;
  scopes?: string[];
}

// Request body types
export interface LoginBody {
  emailOrUsername: string;
  password: string;
}

export interface RequestPasswordResetBody {
  email: string;
  origin: string;
}

export interface ResetPasswordBody {
  token: string;
  password: string;
}

// Auth Provider types
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

export interface EnabledAuthProvider {
  id: string;
  name: string;
  displayName: string;
  type: string;
  icon?: string;
  authUrl?: string;
}

export interface NewProvider {
  name: string;
  displayName: string;
  type: "oidc" | "oauth2";
  icon: string;
  clientId: string;
  clientSecret: string;
  issuerUrl?: string;
  scope?: string;
  authorizationEndpoint?: string;
  tokenEndpoint?: string;
  userInfoEndpoint?: string;
}

export interface UpdateProvidersOrderBody {
  providers: Array<{
    id: string;
    sortOrder: number;
  }>;
}

// Response types using common patterns
export type AuthProvidersResponse = ApiResponse<AuthProvider[]>;
export type EnabledProvidersResponse = ApiResponse<EnabledAuthProvider[]>;
export type AuthProviderResponse = ApiResponse<AuthProvider>;
export type AuthProviderOrderResponse = ApiMessageResponse;
export type Logout200 = SimpleMessageResponse;
export type RequestPasswordReset200 = SimpleMessageResponse;
export type ResetPassword200 = SimpleMessageResponse;

// Axios response types
export type GetEnabledProvidersResult = AxiosResponse<EnabledProvidersResponse>;
export type GetAllProvidersResult = AxiosResponse<AuthProvidersResponse>;
export type CreateProviderResult = AxiosResponse<AuthProviderResponse>;
export type UpdateProviderResult = AxiosResponse<AuthProviderResponse>;
export type DeleteProviderResult = AxiosResponse<ApiMessageResponse>;
export type UpdateProvidersOrderResult = AxiosResponse<AuthProviderOrderResponse>;
export type LoginResult = AxiosResponse<Login200>;
export type LogoutResult = AxiosResponse<Logout200>;
export type RequestPasswordResetResult = AxiosResponse<RequestPasswordReset200>;
export type ResetPasswordResult = AxiosResponse<ResetPassword200>;
export type GetCurrentUserResult = AxiosResponse<GetCurrentUser200>;
export type OIDCConfigResult = AxiosResponse<OidcConfig200>;
export type OIDCConfigData = OidcConfig200;
