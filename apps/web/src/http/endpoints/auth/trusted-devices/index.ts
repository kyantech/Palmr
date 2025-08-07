import apiInstance from "@/config/api";
import {
  RemoveAllTrustedDevicesResponse,
  RemoveTrustedDeviceRequest,
  RemoveTrustedDeviceResponse,
  TrustedDevicesResponse,
} from "./types";

export const getTrustedDevices = async (): Promise<TrustedDevicesResponse> => {
  const response = await apiInstance.get("/api/auth/trusted-devices");
  return response.data;
};

export const removeTrustedDevice = async (data: RemoveTrustedDeviceRequest): Promise<RemoveTrustedDeviceResponse> => {
  const response = await apiInstance.delete(`/api/auth/trusted-devices/${data.deviceId}`);
  return response.data;
};

export const removeAllTrustedDevices = async (): Promise<RemoveAllTrustedDevicesResponse> => {
  const response = await apiInstance.delete("/api/auth/trusted-devices");
  return response.data;
};

export * from "./types";
