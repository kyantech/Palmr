import type { AxiosRequestConfig } from "axios";

import apiInstance from "@/config/api";
import type {
  ActivateReverseShareResult,
  CheckReverseSharePasswordBody,
  CheckReverseSharePasswordResult,
  CreateReverseShareBody,
  CreateReverseShareResult,
  DeactivateReverseShareResult,
  DeleteReverseShareResult,
  GetPresignedUrlBody,
  GetPresignedUrlResult,
  GetReverseShareForUploadParams,
  GetReverseShareForUploadResult,
  GetReverseShareResult,
  ListUserReverseSharesResult,
  RegisterFileUploadBody,
  RegisterFileUploadParams,
  RegisterFileUploadResult,
  UpdateReverseShareBody,
  UpdateReverseShareFileBody,
  UpdateReverseShareFileResult,
  UpdateReverseSharePasswordBody,
  UpdateReverseSharePasswordResult,
  UpdateReverseShareResult,
} from "./types";

/**
 * Create a new reverse share
 * @summary Create Reverse Share
 */
export const createReverseShare = <TData = CreateReverseShareResult>(
  createReverseShareBody: CreateReverseShareBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/create`, createReverseShareBody, options);
};

/**
 * List all reverse shares created by the authenticated user
 * @summary List User's Reverse Shares
 */
export const listUserReverseShares = <TData = ListUserReverseSharesResult>(
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/reverse-shares/list`, options);
};

/**
 * Get a reverse share by ID
 * @summary Get Reverse Share Details
 */
export const getReverseShare = <TData = GetReverseShareResult>(
  id: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/reverse-shares/details/${id}`, options);
};

/**
 * Update a reverse share
 * @summary Update Reverse Share
 */
export const updateReverseShare = <TData = UpdateReverseShareResult>(
  updateReverseShareBody: UpdateReverseShareBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.put(`/api/reverse-shares/update`, updateReverseShareBody, options);
};

/**
 * Update reverse share password
 * @summary Update Reverse Share Password
 */
export const updateReverseSharePassword = <TData = UpdateReverseSharePasswordResult>(
  id: string,
  updateReverseSharePasswordBody: UpdateReverseSharePasswordBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.put(`/api/reverse-shares/password/${id}`, updateReverseSharePasswordBody, options);
};

/**
 * Delete a reverse share
 * @summary Delete Reverse Share
 */
export const deleteReverseShare = <TData = DeleteReverseShareResult>(
  id: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.delete(`/api/reverse-shares/delete/${id}`, options);
};

/**
 * Get reverse share information for upload (public endpoint)
 * @summary Get Reverse Share for Upload (Public)
 */
export const getReverseShareForUpload = <TData = GetReverseShareForUploadResult>(
  id: string,
  params?: GetReverseShareForUploadParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/reverse-shares/upload/${id}`, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Get reverse share information for upload by alias (public endpoint)
 * @summary Get Reverse Share for Upload by Alias (Public)
 */
export const getReverseShareForUploadByAlias = <TData = GetReverseShareForUploadResult>(
  alias: string,
  params?: GetReverseShareForUploadParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/reverse-shares/alias/${alias}/upload`, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Get presigned URL for upload to reverse share (public endpoint)
 * @summary Get Presigned URL for File Upload (Public)
 */
export const getPresignedUrlForUpload = <TData = GetPresignedUrlResult>(
  id: string,
  getPresignedUrlBody: GetPresignedUrlBody,
  params?: RegisterFileUploadParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/presigned-url/${id}`, getPresignedUrlBody, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Get presigned URL for upload to reverse share by alias (public endpoint)
 * @summary Get Presigned URL for File Upload by Alias (Public)
 */
export const getPresignedUrlForUploadByAlias = <TData = GetPresignedUrlResult>(
  alias: string,
  getPresignedUrlBody: GetPresignedUrlBody,
  params?: RegisterFileUploadParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/alias/${alias}/presigned-url`, getPresignedUrlBody, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Register file upload completion (public endpoint)
 * @summary Register File Upload Completion (Public)
 */
export const registerFileUpload = <TData = RegisterFileUploadResult>(
  id: string,
  registerFileUploadBody: RegisterFileUploadBody,
  params?: RegisterFileUploadParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/register-upload/${id}`, registerFileUploadBody, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Register file upload completion by alias (public endpoint)
 * @summary Register File Upload Completion by Alias (Public)
 */
export const registerFileUploadByAlias = <TData = RegisterFileUploadResult>(
  alias: string,
  registerFileUploadBody: RegisterFileUploadBody,
  params?: RegisterFileUploadParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/alias/${alias}/register-file`, registerFileUploadBody, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Verify reverse share password (public endpoint)
 * @summary Verify Reverse Share Password (Public)
 */
export const checkReverseSharePassword = <TData = CheckReverseSharePasswordResult>(
  id: string,
  checkReverseSharePasswordBody: CheckReverseSharePasswordBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/check-password/${id}`, checkReverseSharePasswordBody, options);
};

/**
 * Download file from reverse share
 * @summary Download File from Reverse Share
 */
export const downloadReverseShareFile = <TData = GetPresignedUrlResult>(
  fileId: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/reverse-shares/files/download/${fileId}`, options);
};

/**
 * Delete file from reverse share
 * @summary Delete File from Reverse Share
 */
export const deleteReverseShareFile = <TData = any>(fileId: string, options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.delete(`/api/reverse-shares/files/${fileId}`, options);
};

/**
 * Create or update reverse share alias
 * @summary Create or update reverse share alias
 */
export const createReverseShareAlias = <TData = any>(
  reverseShareId: string,
  createAliasBody: { alias: string },
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/${reverseShareId}/alias`, createAliasBody, options);
};

/**
 * Activate a reverse share
 * @summary Activate Reverse Share
 */
export const activateReverseShare = <TData = ActivateReverseShareResult>(
  id: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.patch(`/api/reverse-shares/activate/${id}`, undefined, options);
};

/**
 * Deactivate a reverse share
 * @summary Deactivate Reverse Share
 */
export const deactivateReverseShare = <TData = DeactivateReverseShareResult>(
  id: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.patch(`/api/reverse-shares/deactivate/${id}`, undefined, options);
};

/**
 * Update file from reverse share
 * @summary Update File from Reverse Share
 */
export const updateReverseShareFile = <TData = UpdateReverseShareFileResult>(
  fileId: string,
  updateReverseShareFileBody: UpdateReverseShareFileBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.put(`/api/reverse-shares/files/${fileId}`, updateReverseShareFileBody, options);
};

/**
 * Copy file from reverse share to user files
 * @summary Copy File from Reverse Share to User Files
 */
export const copyReverseShareFileToUserFiles = <TData = any>(
  fileId: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/files/${fileId}/copy`, undefined, options);
};

/**
 * Create a multipart upload for reverse share (public endpoint)
 * @summary Create Multipart Upload for Reverse Share (Public)
 */
export const createMultipartUploadByAlias = <TData = any>(
  alias: string,
  body: { filename: string; extension: string },
  params?: { password?: string },
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/alias/${alias}/multipart/create`, body, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Get presigned URL for a multipart upload part for reverse share (public endpoint)
 * @summary Get Multipart Part URL for Reverse Share (Public)
 */
export const getMultipartPartUrlByAlias = <TData = any>(
  alias: string,
  params: { uploadId: string; objectName: string; partNumber: string; password?: string },
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/reverse-shares/alias/${alias}/multipart/part-url`, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Complete a multipart upload for reverse share (public endpoint)
 * @summary Complete Multipart Upload for Reverse Share (Public)
 */
export const completeMultipartUploadByAlias = <TData = any>(
  alias: string,
  body: { uploadId: string; objectName: string; parts: Array<{ PartNumber: number; ETag: string }> },
  params?: { password?: string },
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/alias/${alias}/multipart/complete`, body, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Abort a multipart upload for reverse share (public endpoint)
 * @summary Abort Multipart Upload for Reverse Share (Public)
 */
export const abortMultipartUploadByAlias = <TData = any>(
  alias: string,
  body: { uploadId: string; objectName: string },
  params?: { password?: string },
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/reverse-shares/alias/${alias}/multipart/abort`, body, {
    ...options,
    params: { ...params, ...options?.params },
  });
};
