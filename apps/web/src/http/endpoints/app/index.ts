import type { AxiosRequestConfig } from "axios";

import apiInstance from "@/config/api";
import type {
  CheckHealthResult,
  CheckUploadAllowedParams,
  CheckUploadAllowedResult,
  GetAppInfoResult,
  GetDiskSpaceResult,
  RemoveLogoResult,
  UploadLogoBody,
  UploadLogoResult,
} from "./types";

/**
 * Get application base information
 * @summary Get application base information
 */
export const getAppInfo = <TData = GetAppInfoResult>(options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.get(`/api/app/info`, options);
};

/**
 * Upload a new app logo (admin only)
 * @summary Upload app logo
 */
export const uploadLogo = <TData = UploadLogoResult>(
  uploadLogoBody: UploadLogoBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  const formData = new FormData();

  if (uploadLogoBody.file !== undefined) {
    formData.append("file", uploadLogoBody.file as Blob);
  }

  return apiInstance.post(`/api/app/upload-logo`, formData, {
    ...options,
    headers: {
      ...options?.headers,
      "Content-Type": "multipart/form-data",
    },
  });
};

/**
 * Remove the current app logo (admin only)
 * @summary Remove app logo
 */
export const removeLogo = <TData = RemoveLogoResult>(options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.delete(`/api/app/remove-logo`, options);
};

/**
 * Returns the health status of the API
 * @summary Check API Health
 */
export const checkHealth = <TData = CheckHealthResult>(options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.get(`/api/app/health`, options);
};

/**
 * Get server disk space information
 * @summary Get server disk space information
 */
export const getDiskSpace = <TData = GetDiskSpaceResult>(options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.get(`/api/app/disk-space`, options);
};

/**
 * Check if file upload is allowed based on available space (fileSize in bytes)
 * @summary Check if file upload is allowed
 */
export const checkUploadAllowed = <TData = CheckUploadAllowedResult>(
  params: CheckUploadAllowedParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/app/check-upload`, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

export type TestSmtpConnectionResult = { success: boolean; message: string };

export interface TestSmtpConnectionBody {
  smtpConfig?: {
    smtpEnabled: string;
    smtpHost: string;
    smtpPort: string;
    smtpUser: string;
    smtpPass: string;
    smtpSecure: string;
    smtpNoAuth: string;
    smtpTrustSelfSigned: string;
  };
}

export const testSmtpConnection = (
  body?: TestSmtpConnectionBody,
  options?: AxiosRequestConfig
): Promise<{ data: TestSmtpConnectionResult }> => {
  return apiInstance.post(`/api/app/test-smtp`, body || {}, options);
};
