import type { AxiosResponse } from "axios";

export type FieldRequirement = "HIDDEN" | "OPTIONAL" | "REQUIRED";
export type PageLayout = "WETRANSFER" | "DEFAULT";

export interface ReverseShareFile {
  id: string;
  name: string;
  description: string | null;
  extension: string;
  size: string;
  objectName: string;
  uploaderEmail: string | null;
  uploaderName: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface ReverseShareAlias {
  id: string;
  alias: string;
  reverseShareId: string;
  createdAt: string;
  updatedAt: string;
}

export interface BaseReverseShare {
  id: string;
  name: string | null;
  description: string | null;
  expiration: string | null;
  maxFiles: number | null;
  maxFileSize: number | null;
  allowedFileTypes: string | null;
  pageLayout: string;
  isActive: boolean;
  hasPassword: boolean;
  nameFieldRequired: string;
  emailFieldRequired: string;
  createdAt: string;
  updatedAt: string;
  creatorId: string;
  files: ReverseShareFile[];
}

export interface ReverseShareWithAlias extends BaseReverseShare {
  alias?: ReverseShareAlias | null;
}

export interface ReverseShareForUpload {
  id: string;
  name: string | null;
  description: string | null;
  maxFiles: number | null;
  maxFileSize: number | null;
  allowedFileTypes: string | null;
  pageLayout: string;
  hasPassword: boolean;
  currentFileCount: number;
  nameFieldRequired: string;
  emailFieldRequired: string;
}

export interface CreateReverseShare201 {
  reverseShare: BaseReverseShare;
}

export interface UpdateReverseShare200 {
  reverseShare: BaseReverseShare;
}

export interface ListUserReverseShares200 {
  reverseShares: ReverseShareWithAlias[];
}

export interface GetReverseShare200 {
  reverseShare: ReverseShareWithAlias;
}

export interface DeleteReverseShare200 {
  reverseShare: BaseReverseShare;
}

export interface GetReverseShareForUpload200 {
  reverseShare: ReverseShareForUpload;
}

export interface UpdateReverseSharePassword200 {
  reverseShare: BaseReverseShare;
}

export interface GetPresignedUrl200 {
  url: string;
  expiresIn: number;
}

export interface RegisterFileUpload201 {
  file: ReverseShareFile;
}

export interface CheckReverseSharePassword200 {
  valid: boolean;
}

export interface DownloadReverseShareFile200 {
  url: string;
  expiresIn: number;
}

export interface DeleteReverseShareFile200 {
  file: ReverseShareFile;
}

export interface ActivateReverseShare200 {
  reverseShare: BaseReverseShare;
}

export interface DeactivateReverseShare200 {
  reverseShare: BaseReverseShare;
}

export interface UpdateReverseShareFile200 {
  file: ReverseShareFile;
}

export interface CreateReverseShareBody {
  name?: string;
  description?: string;
  expiration?: string;
  maxFiles?: number | null;
  maxFileSize?: number | null;
  allowedFileTypes?: string | null;
  password?: string;
  pageLayout?: PageLayout;
  nameFieldRequired?: FieldRequirement;
  emailFieldRequired?: FieldRequirement;
}

export interface UpdateReverseShareBody {
  id: string;
  name?: string;
  description?: string;
  expiration?: string;
  maxFiles?: number | null;
  maxFileSize?: number | null;
  allowedFileTypes?: string | null;
  password?: string | null;
  pageLayout?: PageLayout;
  isActive?: boolean;
  nameFieldRequired?: FieldRequirement;
  emailFieldRequired?: FieldRequirement;
}

export interface UpdateReverseSharePasswordBody {
  password: string | null;
}

export interface GetPresignedUrlBody {
  objectName: string;
}

export interface RegisterFileUploadBody {
  name: string;
  description?: string;
  extension: string;
  size: number;
  objectName: string;
  uploaderEmail?: string;
  uploaderName?: string;
}

export interface CheckReverseSharePasswordBody {
  password: string;
}

export interface UpdateReverseShareFileBody {
  name?: string;
  description?: string | null;
}

export interface GetReverseShareForUploadParams {
  password?: string;
}

export interface RegisterFileUploadParams {
  password?: string;
}

export type CreateReverseShareResult = AxiosResponse<CreateReverseShare201>;
export type UpdateReverseShareResult = AxiosResponse<UpdateReverseShare200>;
export type ListUserReverseSharesResult = AxiosResponse<ListUserReverseShares200>;
export type GetReverseShareResult = AxiosResponse<GetReverseShare200>;
export type DeleteReverseShareResult = AxiosResponse<DeleteReverseShare200>;
export type GetReverseShareForUploadResult = AxiosResponse<GetReverseShareForUpload200>;
export type UpdateReverseSharePasswordResult = AxiosResponse<UpdateReverseSharePassword200>;
export type GetPresignedUrlResult = AxiosResponse<GetPresignedUrl200>;
export type RegisterFileUploadResult = AxiosResponse<RegisterFileUpload201>;
export type CheckReverseSharePasswordResult = AxiosResponse<CheckReverseSharePassword200>;
export type DownloadReverseShareFileResult = AxiosResponse<DownloadReverseShareFile200>;
export type DeleteReverseShareFileResult = AxiosResponse<DeleteReverseShareFile200>;
export type ActivateReverseShareResult = AxiosResponse<ActivateReverseShare200>;
export type DeactivateReverseShareResult = AxiosResponse<DeactivateReverseShare200>;
export type UpdateReverseShareFileResult = AxiosResponse<UpdateReverseShareFile200>;
