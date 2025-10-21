import type { AxiosResponse } from "axios";

export type ShareAlias = {
  id: string;
  alias: string;
  shareId: string;
  createdAt: string;
  updatedAt: string;
} | null;

export interface ShareFile {
  id: string;
  name: string;
  description: string | null;
  extension: string;
  size: string;
  objectName: string;
  userId: string;
  folderId: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface ShareFolder {
  id: string;
  name: string;
  description: string | null;
  parentId: string | null;
  totalSize: string | null;
  createdAt: string;
  updatedAt: string;
  _count?: {
    files: number;
    children: number;
  };
}

export interface ShareRecipient {
  id: string;
  email: string;
  createdAt: string;
  updatedAt: string;
}

export interface ShareSecurity {
  maxViews: number | null;
  hasPassword: boolean;
}

export interface Share {
  id: string;
  name: string | null;
  description: string | null;
  expiration: string | null;
  views: number;
  createdAt: string;
  updatedAt: string;
  creatorId: string;
  security: ShareSecurity;
  files: ShareFile[];
  folders: ShareFolder[];
  recipients: ShareRecipient[];
  alias: ShareAlias;
}

export interface SimpleShare {
  id: string;
  name: string;
  description: string | null;
}

export interface CreateShare201 {
  share: Share;
}

export interface UpdateShare200 {
  share: Share;
}

export interface UpdateSharePassword200 {
  share: Share;
}

export interface GetShare200 {
  share: Share;
}

export interface GetShareByAlias200 {
  share: Share;
}

export interface DeleteShare200 {
  share: Share;
}

export interface RemoveRecipients200 {
  share: Share;
}

export interface RemoveFiles200 {
  share: Share;
}

export interface AddRecipients200 {
  share: Share;
}

export interface AddFiles200 {
  share: SimpleShare;
}

export interface ListUserShares200 {
  shares: Share[];
}

export interface CreateShareAlias200 {
  alias: {
    id: string;
    alias: string;
    shareId: string;
    createdAt: string;
    updatedAt: string;
  };
}

export interface NotifyRecipients200 {
  message: string;
  notifiedRecipients: string[];
}

export interface CreateShareBody {
  name?: string;
  description?: string;
  expiration?: string;
  files?: string[];
  folders?: string[];
  password?: string;
  maxViews?: number | null;
  recipients?: string[];
}

export interface CreateShareWithFilesBody {
  name?: string;
  description?: string;
  expiration?: string;
  existingFiles?: string[];
  existingFolders?: string[];
  password?: string;
  maxViews?: number | null;
  recipients?: string[];
  folderId?: string | null;
  newFiles?: File[];
}

export interface UpdateShareBody {
  id: string;
  name?: string;
  description?: string;
  expiration?: string;
  password?: string;
  maxViews?: number | null;
  recipients?: string[];
}

export interface UpdateSharePasswordBody {
  password: string | null;
}

export interface AddFilesBody {
  files: string[];
}

export interface RemoveFilesBody {
  files: string[];
}

export interface AddRecipientsBody {
  emails: string[];
}

export interface RemoveRecipientsBody {
  emails: string[];
}

export interface CreateShareAliasBody {
  alias: string;
}

export interface NotifyRecipientsBody {
  shareLink: string;
}

export interface GetShareParams {
  password?: string;
}

export interface GetShareByAliasParams {
  password?: string;
}

export type CreateShareResult = AxiosResponse<CreateShare201>;
export type CreateShareWithFilesResult = AxiosResponse<CreateShare201>;
export type UpdateShareResult = AxiosResponse<UpdateShare200>;
export type ListUserSharesResult = AxiosResponse<ListUserShares200>;
export type GetShareResult = AxiosResponse<GetShare200>;
export type DeleteShareResult = AxiosResponse<DeleteShare200>;
export type UpdateSharePasswordResult = AxiosResponse<UpdateSharePassword200>;
export type AddFilesResult = AxiosResponse<AddFiles200>;
export type RemoveFilesResult = AxiosResponse<RemoveFiles200>;
export type AddRecipientsResult = AxiosResponse<AddRecipients200>;
export type RemoveRecipientsResult = AxiosResponse<RemoveRecipients200>;
export type CreateShareAliasResult = AxiosResponse<CreateShareAlias200>;
export type GetShareByAliasResult = AxiosResponse<GetShareByAlias200>;
export type NotifyRecipientsResult = AxiosResponse<NotifyRecipients200>;
