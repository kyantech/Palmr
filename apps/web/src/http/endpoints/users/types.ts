import type { AxiosResponse } from "axios";

export interface User {
  id: string;
  firstName: string;
  lastName: string;
  username: string;
  email: string;
  image: string | null;
  isAdmin: boolean;
  isActive: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface UserWithMessageResponse {
  user: User;
  message: string;
}

export interface ListUsers200 {
  users: User[];
}

export interface RegisterUserBody {
  firstName: string;
  lastName: string;
  username: string;
  email: string;
  image?: string;
  password: string;
}

export interface UpdateUserBody {
  id: string;
  firstName?: string;
  lastName?: string;
  username?: string;
  email?: string;
  image?: string;
  password?: string;
  isAdmin?: boolean;
}

export interface UpdateUserImageBody {
  image: string;
}

export interface UploadAvatarBody {
  file?: unknown;
}

export type ActivateUser200 = User;
export type DeactivateUser200 = User;
export type DeleteUser200 = User;
export type GetUserById200 = User;
export type UpdateUser200 = User;
export type RemoveAvatar200 = User;
export type UpdateUserImage200 = User;
export type UploadAvatar200 = User;
export type RegisterUser201 = UserWithMessageResponse;

export type RegisterUserResult = AxiosResponse<RegisterUser201>;
export type ListUsersResult = AxiosResponse<User[]>;
export type UpdateUserResult = AxiosResponse<UpdateUser200>;
export type GetUserByIdResult = AxiosResponse<GetUserById200>;
export type DeleteUserResult = AxiosResponse<DeleteUser200>;
export type ActivateUserResult = AxiosResponse<ActivateUser200>;
export type DeactivateUserResult = AxiosResponse<DeactivateUser200>;
export type UpdateUserImageResult = AxiosResponse<UpdateUserImage200>;
export type UploadAvatarResult = AxiosResponse<UploadAvatar200>;
export type RemoveAvatarResult = AxiosResponse<RemoveAvatar200>;
