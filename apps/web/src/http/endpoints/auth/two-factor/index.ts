import apiInstance from "@/config/api";
import type {
  CompleteTwoFactorLoginRequest,
  DisableTwoFactorRequest,
  DisableTwoFactorResponse,
  GenerateBackupCodesResponse,
  TwoFactorSetupRequest,
  TwoFactorSetupResponse,
  TwoFactorStatus,
  VerifySetupRequest,
  VerifySetupResponse,
  VerifyTokenRequest,
  VerifyTokenResponse,
} from "./types";

export const generate2FASetup = async (data?: TwoFactorSetupRequest) => {
  return apiInstance.post<TwoFactorSetupResponse>("/api/auth/2fa/setup", data);
};

export const verifyTwoFactorSetup = async (data: VerifySetupRequest) => {
  return apiInstance.post<VerifySetupResponse>("/api/auth/2fa/verify-setup", data);
};

export const verifyTwoFactorToken = async (data: VerifyTokenRequest) => {
  return apiInstance.post<VerifyTokenResponse>("/api/auth/2fa/verify", data);
};

export const disableTwoFactor = async (data: DisableTwoFactorRequest) => {
  return apiInstance.post<DisableTwoFactorResponse>("/api/auth/2fa/disable", data);
};

export const generateBackupCodes = async () => {
  return apiInstance.post<GenerateBackupCodesResponse>("/api/auth/2fa/backup-codes");
};

export const getTwoFactorStatus = async () => {
  return apiInstance.get<TwoFactorStatus>("/api/auth/2fa/status");
};

export const completeTwoFactorLogin = async (data: CompleteTwoFactorLoginRequest) => {
  return apiInstance.post("/api/auth/2fa/login", data);
};
