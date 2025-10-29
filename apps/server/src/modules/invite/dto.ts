import { z } from "zod";

export const CreateInviteTokenResponseSchema = z.object({
  token: z.string().describe("Invite token"),
  expiresAt: z.coerce.date().describe("Token expiration date"),
});

export const ValidateInviteTokenResponseSchema = z.object({
  valid: z.boolean().describe("Whether the token is valid"),
  used: z.boolean().optional().describe("Whether the token has been used"),
  expired: z.boolean().optional().describe("Whether the token has expired"),
});

export const RegisterWithInviteSchema = z.object({
  token: z.string().min(1, "Token is required").describe("Invite token"),
  firstName: z.string().min(1, "First name is required").describe("User first name"),
  lastName: z.string().min(1, "Last name is required").describe("User last name"),
  username: z.string().min(3, "Username must be at least 3 characters").describe("User username"),
  email: z.string().email("Invalid email").describe("User email"),
  password: z.string().min(8, "Password must be at least 8 characters").describe("User password"),
});

export const RegisterWithInviteResponseSchema = z.object({
  message: z.string().describe("Success message"),
  user: z.object({
    id: z.string().describe("User ID"),
    username: z.string().describe("User username"),
    email: z.string().email().describe("User email"),
  }),
});
