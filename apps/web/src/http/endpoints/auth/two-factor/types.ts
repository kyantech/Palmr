export interface TwoFactorSetupRequest {
  appName?: string;
}

export interface BackupCode {
  code: string;
  used: boolean;
}

export interface TwoFactorSetupResponse {
  secret: string;
  qrCode: string;
  manualEntryKey: string;
  backupCodes: BackupCode[];
}

export interface VerifySetupRequest {
  token: string;
  secret: string;
}

export interface VerifySetupResponse {
  success: boolean;
  backupCodes: string[];
}

export interface VerifyTokenRequest {
  token: string;
}

export interface VerifyTokenResponse {
  success: boolean;
  method: "totp" | "backup";
}

export interface DisableTwoFactorRequest {
  password: string;
}

export interface DisableTwoFactorResponse {
  success: boolean;
}

export interface GenerateBackupCodesResponse {
  backupCodes: string[];
}

export interface TwoFactorStatus {
  enabled: boolean;
  verified: boolean;
  availableBackupCodes: number;
}

export interface CompleteTwoFactorLoginRequest {
  userId: string;
  token: string;
  rememberDevice?: boolean;
}

export interface LoginResponse {
  user?: {
    id: string;
    firstName: string;
    lastName: string;
    username: string;
    email: string;
    image?: string | null;
    isAdmin: boolean;
    isActive: boolean;
    createdAt: string;
    updatedAt: string;
  };
  requiresTwoFactor?: boolean;
  userId?: string;
  message?: string;
}
