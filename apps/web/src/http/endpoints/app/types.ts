import type { AxiosResponse } from "axios";

// Base types that are reused across different operations
export interface FileSizeInfo {
  bytes: number;
  kb: number;
  mb: number;
  gb: number;
}

export interface DiskSpaceInfo {
  diskSizeGB: number;
  diskUsedGB: number;
  diskAvailableGB: number;
  uploadAllowed: boolean;
}

// Response types using base types
export interface CheckHealth200 {
  status: string;
  timestamp: string;
}

export interface CheckUploadAllowed200 extends DiskSpaceInfo {
  fileSizeInfo: FileSizeInfo;
}

export type GetDiskSpace200 = DiskSpaceInfo;

export interface GetAppInfo200 {
  appName: string;
  appDescription: string;
  appLogo: string;
  firstUserAccess: boolean;
}

export interface RemoveLogo200 {
  message: string;
}

export interface UploadLogo200 {
  logo: string;
}

// Request body and parameter types
export interface CheckUploadAllowedParams {
  fileSize: string;
}

export interface UploadLogoBody {
  file?: unknown;
}

// Axios response types
export type GetAppInfoResult = AxiosResponse<GetAppInfo200>;
export type UploadLogoResult = AxiosResponse<UploadLogo200>;
export type RemoveLogoResult = AxiosResponse<RemoveLogo200>;
export type CheckHealthResult = AxiosResponse<CheckHealth200>;
export type GetDiskSpaceResult = AxiosResponse<GetDiskSpace200>;
export type CheckUploadAllowedResult = AxiosResponse<CheckUploadAllowed200>;
