import { z } from "zod";

import { ConfigService } from "../config/service";

const configService = new ConfigService();

export const createPasswordSchema = async () => {
  const minLength = Number(await configService.getValue("passwordMinLength"));
  return z.string().min(minLength, `Password must be at least ${minLength} characters`).describe("User password");
};

export const LoginSchema = z.object({
  emailOrUsername: z.string().min(1, "Email or username is required").describe("User email or username"),
  password: z.string().min(6, "Password must be at least 6 characters").describe("User password"),
});
export type LoginInput = z.infer<typeof LoginSchema>;

export const RequestPasswordResetSchema = z.object({
  email: z.string().email("Invalid email").describe("User email"),
  origin: z.string().url("Invalid origin").describe("Origin of the request"),
});

export const BaseResetPasswordSchema = z.object({
  token: z.string().min(1, "Token is required").describe("Reset password token"),
});

export type BaseResetPasswordInput = z.infer<typeof BaseResetPasswordSchema>;

export const createResetPasswordSchema = async () => {
  const minLength = Number(await configService.getValue("passwordMinLength"));
  return BaseResetPasswordSchema.extend({
    password: z.string().min(minLength, `Password must be at least ${minLength} characters`).describe("User password"),
  });
};

export type ResetPasswordInput = BaseResetPasswordInput & {
  password: string;
};
