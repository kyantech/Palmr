import type { AxiosRequestConfig } from "axios";

import apiInstance from "@/config/api";
import type {
  BulkUpdateConfigsBody,
  BulkUpdateConfigsResult,
  GetAllConfigsResult,
  UpdateConfigBody,
  UpdateConfigResult,
} from "./types";

/**
 * Update a configuration value (admin only)
 * @summary Update a configuration value
 */
export const updateConfig = <TData = UpdateConfigResult>(
  key: string,
  updateConfigBody: UpdateConfigBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.patch(`/api/config/update/${key}`, updateConfigBody, options);
};

/**
 * List all configurations (admin only)
 * @summary List all configurations
 */
export const getAllConfigs = <TData = GetAllConfigsResult>(options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.get(`/api/app/configs`, options);
};

/**
 * Bulk update configuration values (admin only)
 * @summary Bulk update configuration values
 */
export const bulkUpdateConfigs = <TData = BulkUpdateConfigsResult>(
  bulkUpdateConfigsBody: BulkUpdateConfigsBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.patch(`api/config/update/bulk`, bulkUpdateConfigsBody, options);
};
