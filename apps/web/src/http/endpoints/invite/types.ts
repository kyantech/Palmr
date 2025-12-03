export interface GenerateInviteTokenResponse {
  token: string;
  expiresAt: string;
}

export interface ValidateInviteTokenResponse {
  valid: boolean;
  used?: boolean;
  expired?: boolean;
}

export interface RegisterWithInviteRequest {
  token: string;
  firstName: string;
  lastName: string;
  username: string;
  email: string;
  password: string;
}

export interface RegisterWithInviteResponse {
  message: string;
  user: {
    id: string;
    username: string;
    email: string;
  };
}
