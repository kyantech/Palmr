import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { ConfigService } from "../config/service";
import { validatePasswordMiddleware } from "../user/middleware";
import { AuthController } from "./controller";
import { CompleteTwoFactorLoginSchema, createResetPasswordSchema, RequestPasswordResetSchema } from "./dto";

const configService = new ConfigService();

const createPasswordSchema = async () => {
  const minLength = Number(await configService.getValue("passwordMinLength"));
  return z.string().min(minLength, `Password must be at least ${minLength} characters`).describe("User password");
};

export async function authRoutes(app: FastifyInstance) {
  const authController = new AuthController();

  const passwordSchema = await createPasswordSchema();
  const loginSchema = z.object({
    emailOrUsername: z.string().min(1, "Email or username is required").describe("User email or username"),
    password: passwordSchema,
  });

  app.post(
    "/auth/login",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "login",
        summary: "Login",
        description: "Performs login and returns user data",
        body: loginSchema,
        response: {
          200: z.union([
            z.object({
              user: z.object({
                id: z.string().describe("User ID"),
                firstName: z.string().describe("User first name"),
                lastName: z.string().describe("User last name"),
                username: z.string().describe("User username"),
                email: z.string().email().describe("User email"),
                isAdmin: z.boolean().describe("User is admin"),
                isActive: z.boolean().describe("User is active"),
                createdAt: z.date().describe("User creation date"),
                updatedAt: z.date().describe("User last update date"),
              }),
            }),
            z.object({
              requiresTwoFactor: z.boolean().describe("Whether 2FA is required"),
              userId: z.string().describe("User ID for 2FA verification"),
              message: z.string().describe("2FA required message"),
            }),
          ]),
          400: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    authController.login.bind(authController)
  );

  app.post(
    "/auth/2fa/login",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "completeTwoFactorLogin",
        summary: "Complete Two-Factor Login",
        description: "Complete login process with 2FA verification",
        body: CompleteTwoFactorLoginSchema,
        response: {
          200: z.object({
            user: z.object({
              id: z.string().describe("User ID"),
              firstName: z.string().describe("User first name"),
              lastName: z.string().describe("User last name"),
              username: z.string().describe("User username"),
              email: z.string().email().describe("User email"),
              isAdmin: z.boolean().describe("User is admin"),
              isActive: z.boolean().describe("User is active"),
              createdAt: z.date().describe("User creation date"),
              updatedAt: z.date().describe("User last update date"),
            }),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    authController.completeTwoFactorLogin.bind(authController)
  );

  app.post(
    "/auth/logout",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "logout",
        summary: "Logout",
        description: "Performs logout by clearing the token cookie",
        response: {
          200: z.object({ message: z.string().describe("Logout message") }),
        },
      },
    },
    authController.logout.bind(authController)
  );

  app.post(
    "/auth/forgot-password",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "requestPasswordReset",
        summary: "Request Password Reset",
        description: "Request password reset email",
        body: RequestPasswordResetSchema,
        response: {
          200: z.object({
            message: z.string().describe("Reset password email sent"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    authController.requestPasswordReset.bind(authController)
  );

  app.post(
    "/auth/reset-password",
    {
      preValidation: validatePasswordMiddleware,
      schema: {
        tags: ["Authentication"],
        operationId: "resetPassword",
        summary: "Reset Password",
        description: "Reset password using token",
        body: await createResetPasswordSchema(),
        response: {
          200: z.object({
            message: z.string().describe("Reset password message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    authController.resetPassword.bind(authController)
  );

  app.get(
    "/auth/me",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "getCurrentUser",
        summary: "Get Current User",
        description: "Returns the current authenticated user's information or null if not authenticated",
        response: {
          200: z.union([
            z.object({
              user: z.object({
                id: z.string().describe("User ID"),
                firstName: z.string().describe("User first name"),
                lastName: z.string().describe("User last name"),
                username: z.string().describe("User username"),
                email: z.string().email().describe("User email"),
                image: z.string().nullable().describe("User profile image URL"),
                isAdmin: z.boolean().describe("User is admin"),
                isActive: z.boolean().describe("User is active"),
                createdAt: z.date().describe("User creation date"),
                updatedAt: z.date().describe("User last update date"),
              }),
            }),
            z.object({
              user: z.null().describe("No user when not authenticated"),
            }),
          ]),
        },
      },
    },
    authController.getCurrentUser.bind(authController)
  );

  app.get(
    "/auth/trusted-devices",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "getTrustedDevices",
        summary: "Get Trusted Devices",
        description: "Get all trusted devices for the current user",
        response: {
          200: z.object({
            devices: z.array(
              z.object({
                id: z.string().describe("Device ID"),
                deviceName: z.string().nullable().describe("Device name"),
                userAgent: z.string().nullable().describe("User agent"),
                ipAddress: z.string().nullable().describe("IP address"),
                createdAt: z.date().describe("Creation date"),
                lastUsedAt: z.date().describe("Last used date"),
                expiresAt: z.date().describe("Expiration date"),
              })
            ),
          }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
      preValidation: async (request: FastifyRequest, reply: FastifyReply) => {
        try {
          await request.jwtVerify();
        } catch (err) {
          console.error(err);
          reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
        }
      },
    },
    authController.getTrustedDevices.bind(authController)
  );

  app.delete(
    "/auth/trusted-devices/:id",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "removeTrustedDevice",
        summary: "Remove Trusted Device",
        description: "Remove a specific trusted device",
        params: z.object({
          id: z.string().describe("Device ID"),
        }),
        response: {
          200: z.object({
            success: z.boolean().describe("Success status"),
            message: z.string().describe("Success message"),
          }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
      preValidation: async (request: FastifyRequest, reply: FastifyReply) => {
        try {
          await request.jwtVerify();
        } catch (err) {
          console.error(err);
          reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
        }
      },
    },
    authController.removeTrustedDevice.bind(authController)
  );

  app.delete(
    "/auth/trusted-devices",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "removeAllTrustedDevices",
        summary: "Remove All Trusted Devices",
        description: "Remove all trusted devices for the current user",
        response: {
          200: z.object({
            success: z.boolean().describe("Success status"),
            message: z.string().describe("Success message"),
            removedCount: z.number().describe("Number of devices removed"),
          }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
      preValidation: async (request: FastifyRequest, reply: FastifyReply) => {
        try {
          await request.jwtVerify();
        } catch (err) {
          console.error(err);
          reply.status(401).send({ error: "Unauthorized: a valid token is required to access this resource." });
        }
      },
    },
    authController.removeAllTrustedDevices.bind(authController)
  );

  app.get(
    "/auth/config",
    {
      schema: {
        tags: ["Authentication"],
        operationId: "getAuthConfig",
        summary: "Get Authentication Configuration",
        description: "Get authentication configuration settings",
        response: {
          200: z.object({
            passwordAuthEnabled: z.boolean().describe("Whether password authentication is enabled"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    authController.getAuthConfig.bind(authController)
  );
}
