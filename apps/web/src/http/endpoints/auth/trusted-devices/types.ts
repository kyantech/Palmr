export interface TrustedDevice {
  id: string;
  deviceName: string | null;
  userAgent: string | null;
  ipAddress: string | null;
  createdAt: string;
  lastUsedAt: string;
  expiresAt: string;
}

export interface TrustedDevicesResponse {
  devices: TrustedDevice[];
}

export interface RemoveTrustedDeviceRequest {
  deviceId: string;
}

export interface RemoveTrustedDeviceResponse {
  success: boolean;
  message: string;
}

export interface RemoveAllTrustedDevicesResponse {
  success: boolean;
  message: string;
  removedCount: number;
}
