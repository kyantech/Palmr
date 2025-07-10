import type { AxiosResponse } from "axios";

// Base types that are reused across different operations
export interface ConfigItem {
  key: string;
  value: string;
  type: string;
  group: string;
  updatedAt: string;
}

export interface ConfigUpdateItem {
  key: string;
  value: string;
}

// Response types using base types
export interface UpdateConfig200 {
  config: ConfigItem;
}

export interface GetAllConfigs200 {
  configs: ConfigItem[];
}

export interface BulkUpdateConfigs200 {
  configs: ConfigItem[];
}

// Request body types
export interface UpdateConfigBody {
  value: string;
}

export type BulkUpdateConfigsBody = ConfigUpdateItem[];

// Axios response types
export type UpdateConfigResult = AxiosResponse<UpdateConfig200>;
export type GetAllConfigsResult = AxiosResponse<GetAllConfigs200>;
export type BulkUpdateConfigsResult = AxiosResponse<BulkUpdateConfigs200>;
