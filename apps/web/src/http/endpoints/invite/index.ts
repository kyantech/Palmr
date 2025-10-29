import type { AxiosRequestConfig } from "axios";

import apiInstance from "@/config/api";
import type {
  GenerateInviteTokenResponse,
  RegisterWithInviteRequest,
  RegisterWithInviteResponse,
  ValidateInviteTokenResponse,
} from "./types";

export const generateInviteToken = async <TData = GenerateInviteTokenResponse>(
  options?: AxiosRequestConfig
): Promise<TData> => {
  const response = await apiInstance.post(`/api/invite-tokens`, undefined, options);
  return response.data;
};

export const validateInviteToken = async <TData = ValidateInviteTokenResponse>(
  token: string,
  options?: AxiosRequestConfig
): Promise<TData> => {
  const response = await apiInstance.get(`/api/invite-tokens/${token}`, options);
  return response.data;
};

export const registerWithInvite = <TData = RegisterWithInviteResponse>(
  data: RegisterWithInviteRequest,
  options?: AxiosRequestConfig
): Promise<TData> => {
  return apiInstance.post(`/api/register-with-invite`, data, options);
};
