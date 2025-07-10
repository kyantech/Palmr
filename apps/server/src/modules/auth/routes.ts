import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { ConfigService } from "../config/service";
import { validatePasswordMiddleware } from "../user/middleware";
import { AuthController } from "./controller";
import { createResetPasswordSchema, RequestPasswordResetSchema } from "./dto";

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
    authController.login.bind(authController)
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
        description: "Returns the current authenticated user's information",
        response: {
          200: z.object({
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
    authController.getCurrentUser.bind(authController)
  );
}
