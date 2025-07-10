import type { AxiosResponse } from "axios";

// Base types that are reused across different operations
export interface FileItem {
  id: string;
  name: string;
  description: string | null;
  extension: string;
  size: string;
  objectName: string;
  userId: string;
  createdAt: string;
  updatedAt: string;
}

export interface FileOperationRequest {
  name: string;
  description?: string;
  extension: string;
  size: number;
  objectName: string;
}

// Common response patterns
export interface FileOperationResponse {
  file: FileItem;
  message: string;
}

export interface MessageOnlyResponse {
  message: string;
}

export interface UrlResponse {
  url: string;
}

export interface PresignedUrlResponse extends UrlResponse {
  objectName: string;
}

export interface DownloadUrlResponse extends UrlResponse {
  expiresIn: number;
}

// Response types using base types
export interface ListFiles200 {
  files: FileItem[];
}

// Request body types
export type CheckFileBody = FileOperationRequest;
export type RegisterFileBody = FileOperationRequest;

export interface UpdateFileBody {
  name?: string;
  description?: string | null;
}

// Query parameter types
export interface GetPresignedUrlParams {
  filename: string;
  extension: string;
}

export type RegisterFile201 = FileOperationResponse;
export type UpdateFile200 = FileOperationResponse;
export type DeleteFile200 = MessageOnlyResponse;
export type CheckFile201 = MessageOnlyResponse;
export type GetPresignedUrl200 = PresignedUrlResponse;
export type GetDownloadUrl200 = DownloadUrlResponse;

// Axios response types
export type GetPresignedUrlResult = AxiosResponse<GetPresignedUrl200>;
export type RegisterFileResult = AxiosResponse<RegisterFile201>;
export type CheckFileResult = AxiosResponse<CheckFile201>;
export type ListFilesResult = AxiosResponse<ListFiles200>;
export type GetDownloadUrlResult = AxiosResponse<GetDownloadUrl200>;
export type DeleteFileResult = AxiosResponse<DeleteFile200>;
export type UpdateFileResult = AxiosResponse<UpdateFile200>;
