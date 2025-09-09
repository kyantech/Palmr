import type { AxiosResponse } from "axios";

export interface FolderItem {
  id: string;
  name: string;
  description: string | null;
  parentId: string | null;
  userId: string;
  createdAt: string;
  updatedAt: string;
  totalSize?: string;
  _count?: {
    files: number;
    children: number;
  };
}

export interface FolderOperationRequest {
  name: string;
  description?: string;
  objectName: string;
  parentId?: string;
}

export interface UpdateFolderBody {
  name?: string;
  description?: string | null;
}

export interface MoveFolderBody {
  parentId: string | null;
}

export interface FolderOperationResponse {
  folder: FolderItem;
  message: string;
}

export interface MessageOnlyResponse {
  message: string;
}

export interface ListFolders200 {
  folders: FolderItem[];
}

export type CheckFolderBody = FolderOperationRequest;
export type RegisterFolderBody = FolderOperationRequest;

export type CheckFolder201 = MessageOnlyResponse;
export type RegisterFolder201 = FolderOperationResponse;
export type UpdateFolder200 = FolderOperationResponse;
export type MoveFolder200 = FolderOperationResponse;
export type DeleteFolder200 = MessageOnlyResponse;

export type CheckFolderResult = AxiosResponse<CheckFolder201>;
export type RegisterFolderResult = AxiosResponse<RegisterFolder201>;
export type ListFoldersResult = AxiosResponse<ListFolders200>;
export type UpdateFolderResult = AxiosResponse<UpdateFolder200>;
export type MoveFolderResult = AxiosResponse<MoveFolder200>;
export type DeleteFolderResult = AxiosResponse<DeleteFolder200>;
