import type { AxiosRequestConfig } from "axios";

import apiInstance from "@/config/api";
import type {
  AddFilesBody,
  AddFilesResult,
  AddRecipientsBody,
  AddRecipientsResult,
  CreateShareAliasBody,
  CreateShareAliasResult,
  CreateShareBody,
  CreateShareResult,
  CreateShareWithFilesBody,
  CreateShareWithFilesResult,
  DeleteShareResult,
  GetShareByAliasParams,
  GetShareByAliasResult,
  GetShareParams,
  GetShareResult,
  ListUserSharesResult,
  NotifyRecipientsBody,
  NotifyRecipientsResult,
  RemoveFilesBody,
  RemoveFilesResult,
  RemoveRecipientsBody,
  RemoveRecipientsResult,
  UpdateShareBody,
  UpdateSharePasswordBody,
  UpdateSharePasswordResult,
  UpdateShareResult,
} from "./types";

/**
 * Create a new share
 * @summary Create a new share
 */
export const createShare = <TData = CreateShareResult>(
  createShareBody: CreateShareBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/shares/create`, createShareBody, options);
};

/**
 * Create a new share with file uploads
 * @summary Create a new share and upload files in a single action
 */
export const createShareWithFiles = <TData = CreateShareWithFilesResult>(
  createShareWithFilesBody: CreateShareWithFilesBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  const formData = new FormData();

  // Add text fields
  if (createShareWithFilesBody.name) {
    formData.append("name", createShareWithFilesBody.name);
  }
  if (createShareWithFilesBody.description) {
    formData.append("description", createShareWithFilesBody.description);
  }
  if (createShareWithFilesBody.expiration) {
    formData.append("expiration", createShareWithFilesBody.expiration);
  }
  if (createShareWithFilesBody.password) {
    formData.append("password", createShareWithFilesBody.password);
  }
  if (createShareWithFilesBody.maxViews !== undefined && createShareWithFilesBody.maxViews !== null) {
    formData.append("maxViews", createShareWithFilesBody.maxViews.toString());
  }
  if (createShareWithFilesBody.folderId !== undefined) {
    formData.append("folderId", createShareWithFilesBody.folderId || "");
  }

  // Add array fields as JSON strings
  if (createShareWithFilesBody.existingFiles && createShareWithFilesBody.existingFiles.length > 0) {
    formData.append("existingFiles", JSON.stringify(createShareWithFilesBody.existingFiles));
  }
  if (createShareWithFilesBody.existingFolders && createShareWithFilesBody.existingFolders.length > 0) {
    formData.append("existingFolders", JSON.stringify(createShareWithFilesBody.existingFolders));
  }
  if (createShareWithFilesBody.recipients && createShareWithFilesBody.recipients.length > 0) {
    formData.append("recipients", JSON.stringify(createShareWithFilesBody.recipients));
  }

  // Add files
  if (createShareWithFilesBody.newFiles && createShareWithFilesBody.newFiles.length > 0) {
    createShareWithFilesBody.newFiles.forEach((file) => {
      formData.append("files", file);
    });
  }

  return apiInstance.post(`/api/shares/create-with-files`, formData, {
    ...options,
    headers: {
      ...options?.headers,
      "Content-Type": "multipart/form-data",
    },
  });
};

/**
 * Update a share
 * @summary Update a share
 */
export const updateShare = <TData = UpdateShareResult>(
  updateShareBody: UpdateShareBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.put(`/api/shares/update`, updateShareBody, options);
};

/**
 * List all shares created by the authenticated user
 * @summary List all shares created by the authenticated user
 */
export const listUserShares = <TData = ListUserSharesResult>(options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.get(`/api/shares/list`, options);
};

/**
 * Get a share by ID
 * @summary Get a share by ID
 */
export const getShare = <TData = GetShareResult>(
  shareId: string,
  params?: GetShareParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/shares/details/${shareId}`, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Delete a share
 * @summary Delete a share
 */
export const deleteShare = <TData = DeleteShareResult>(id: string, options?: AxiosRequestConfig): Promise<TData> => {
  return apiInstance.delete(`/api/shares/delete/${id}`, options);
};

/**
 * @summary Update share password
 */
export const updateSharePassword = <TData = UpdateSharePasswordResult>(
  shareId: string,
  updateSharePasswordBody: UpdateSharePasswordBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.patch(`api/shares/password/update/${shareId}`, updateSharePasswordBody, options);
};

/**
 * @summary Add files to share
 */
export const addFiles = <TData = AddFilesResult>(
  shareId: string,
  addFilesBody: AddFilesBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/shares/files/add/${shareId}`, addFilesBody, options);
};

/**
 * @summary Remove files from share
 */
export const removeFiles = <TData = RemoveFilesResult>(
  shareId: string,
  removeFilesBody: RemoveFilesBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.delete(`/api/shares/files/remove/${shareId}`, {
    data: removeFilesBody,
    ...options,
  });
};

/**
 * @summary Add recipients to a share
 */
export const addRecipients = <TData = AddRecipientsResult>(
  shareId: string,
  addRecipientsBody: AddRecipientsBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/shares/recipients/add/${shareId}`, addRecipientsBody, options);
};

/**
 * Remove recipients from a share
 * @summary Remove recipients from a share
 */
export const removeRecipients = <TData = RemoveRecipientsResult>(
  shareId: string,
  removeRecipientsBody: RemoveRecipientsBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.delete(`/api/shares/recipients/remove/${shareId}`, {
    data: removeRecipientsBody,
    ...options,
  });
};

/**
 * @summary Create or update share alias
 */
export const createShareAlias = <TData = CreateShareAliasResult>(
  shareId: string,
  createShareAliasBody: CreateShareAliasBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/shares/alias/create/${shareId}`, createShareAliasBody, options);
};

/**
 * @summary Get share by alias
 */
export const getShareByAlias = <TData = GetShareByAliasResult>(
  alias: string,
  params?: GetShareByAliasParams,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/shares/alias/get/${alias}`, {
    ...options,
    params: { ...params, ...options?.params },
  });
};

/**
 * Send email notification with share link to all recipients
 * @summary Send email notification to share recipients
 */
export const notifyRecipients = <TData = NotifyRecipientsResult>(
  shareId: string,
  notifyRecipientsBody: NotifyRecipientsBody,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/shares/recipients/notify/${shareId}`, notifyRecipientsBody, options);
};

/**
 * @summary Add folders to share
 */
export const addFolders = <TData = any>(
  shareId: string,
  addFoldersBody: { folders: string[] },
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/shares/folders/add/${shareId}`, addFoldersBody, options);
};

/**
 * @summary Remove folders from share
 */
export const removeFolders = <TData = any>(
  shareId: string,
  removeFoldersBody: { folders: string[] },
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.delete(`/api/shares/folders/remove/${shareId}`, {
    data: removeFoldersBody,
    ...options,
  });
};

/**
 * @summary Get folder contents within a share
 */
export const getShareFolderContents = <TData = any>(
  shareId: string,
  folderId: string,
  password?: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.get(`/api/shares/${shareId}/folders/${folderId}/contents`, {
    ...options,
    params: { password, ...options?.params },
  });
};
