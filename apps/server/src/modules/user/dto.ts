import { z } from "zod";

import { ConfigService } from "../config/service";

const configService = new ConfigService();

export const BaseRegisterUserSchema = z.object({
  firstName: z.string().min(1),
  lastName: z.string().min(1),
  username: z.string().min(3),
  email: z.string().email(),
  image: z.string().optional(),
  isAdmin: z.boolean().optional().default(false),
});

export type BaseRegisterUserInput = z.infer<typeof BaseRegisterUserSchema>;

export const createRegisterUserSchema = async () => {
  const minLength = Number(await configService.getValue("passwordMinLength"));
  return BaseRegisterUserSchema.extend({
    password: z.string().min(minLength, `Password must be at least ${minLength} characters`),
  });
};

export type RegisterUserInput = BaseRegisterUserInput & {
  password: string;
  isAdmin?: boolean;
};

export const UpdateUserSchema = z.object({
  id: z.string(),
  firstName: z.string().min(1).optional(),
  lastName: z.string().min(1).optional(),
  username: z.string().min(3).optional(),
  email: z.string().email().optional(),
  image: z.string().optional(),
  password: z.string().optional(),
  isAdmin: z.boolean().optional(),
});

export type UpdateUserInput = z.infer<typeof UpdateUserSchema>;

export const UserResponseSchema = z.object({
  id: z.string(),
  firstName: z.string(),
  lastName: z.string(),
  username: z.string(),
  email: z.string(),
  image: z.string().nullable(),
  isAdmin: z.boolean(),
  isActive: z.boolean(),
  createdAt: z.date(),
  updatedAt: z.date(),
});

export type UserResponse = z.infer<typeof UserResponseSchema>;
