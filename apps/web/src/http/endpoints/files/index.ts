import type { AxiosRequestConfig } from "axios";

import apiInstance from "@/config/api";
import type {
  CheckFileBody,
  CheckFileResult,
  DeleteFileResult,
  GetDownloadUrlResult,
  GetPresignedUrlParams,
  GetPresignedUrlResult,
  ListFilesResult,
  MoveFileBody,
  MoveFileResult,
  RegisterFileBody,
  RegisterFileResult,
  UpdateFileBody,
  UpdateFileResult,
} from "./types";

/**
 * Generates a pre-signed URL for direct upload to S3-compatible storage
 * @summary Get Presigned URL for File
 */
export const getFilePresignedUrl = <TData = GetPresignedUrlResult>(
  params: GetPresignedUrlParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/files/presigned-url`, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Checks if the file meets constraints like MAX_FILESIZE
 * @summary Check file for constraints
 */
export const checkFile = <TData = CheckFileResult>(
  CheckFileBody: CheckFileBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/files/check`, CheckFileBody, options);
};

/**
 * Registers file metadata in the database
 * @summary Register File Metadata
 */
export const registerFile = <TData = RegisterFileResult>(
  registerFileBody: RegisterFileBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/files`, registerFileBody, options);
};

/**
 * Lists user files
 * @summary List Files
 */
export const listFiles = <TData = ListFilesResult>(
  params: { folderId?: string; recursive?: boolean } = {},
  options?: AxiosRequestConfig
): Promise<TData> => {
  // Convert boolean recursive to string as expected by server
  const queryParams = {
    ...params,
    recursive: params.recursive !== undefined ? params.recursive.toString() : undefined,
  };

  return apiInstance.get(`/api/files`, {
    ...options,
    params: { ...queryParams, ...options?.params },
  });
};

/**
 * Generates a pre-signed URL for downloading a private file
 * @summary Get Download URL
 */
export const getDownloadUrl = <TData = GetDownloadUrlResult>(
  objectName: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/files/download/${objectName}`, options);
};

/**
 * Deletes a user file
 * @summary Delete File
 */
export const deleteFile = <TData = DeleteFileResult>(id: string, options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.delete(`/api/files/${id}`, options);
};

/**
 * Updates file metadata in the database
 * @summary Update File Metadata
 */
export const updateFile = <TData = UpdateFileResult>(
  id: string,
  updateFileBody: UpdateFileBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.patch(`/api/files/${id}`, updateFileBody, options);
};

/**
 * Moves a file to a different folder
 * @summary Move File
 */
export const moveFile = <TData = MoveFileResult>(
  id: string,
  moveFileBody: MoveFileBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.put(`/api/files/${id}/move`, moveFileBody, options);
};
