import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { ShareController } from "./controller";
import {
  CreateShareSchema,
  ShareAliasResponseSchema,
  ShareResponseSchema,
  UpdateShareItemsSchema,
  UpdateSharePasswordSchema,
  UpdateShareRecipientsSchema,
  UpdateShareSchema,
} from "./dto";

export async function shareRoutes(app: FastifyInstance) {
  const shareController = new ShareController();

  const preValidation = async (request: FastifyRequest, reply: FastifyReply) => {
    try {
      await request.jwtVerify();
    } catch (err) {
      console.error(err);
      reply.status(401).send({ error: "Token inválido ou ausente." });
    }
  };

  app.post(
    "/shares",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "createShare",
        summary: "Create a new share",
        description: "Create a new share with files and/or folders",
        body: CreateShareSchema,
        response: {
          201: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.createShare.bind(shareController)
  );

  app.get(
    "/shares/me",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "listUserShares",
        summary: "List all shares created by the authenticated user",
        description: "List all shares created by the authenticated user",
        response: {
          200: z.object({
            shares: z.array(ShareResponseSchema),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.listUserShares.bind(shareController)
  );

  app.get(
    "/shares/:shareId",
    {
      schema: {
        tags: ["Share"],
        operationId: "getShare",
        summary: "Get a share by ID",
        description: "Get a share by ID",
        params: z.object({
          shareId: z.string().describe("The share ID"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("The share password"),
        }),
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.getShare.bind(shareController)
  );

  app.put(
    "/shares",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "updateShare",
        summary: "Update a share",
        description: "Update a share",
        body: UpdateShareSchema,
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.updateShare.bind(shareController)
  );

  app.delete(
    "/shares/:id",
    {
      schema: {
        tags: ["Share"],
        operationId: "deleteShare",
        summary: "Delete a share",
        description: "Delete a share",
        params: z.object({
          id: z.string().describe("The share ID"),
        }),
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.deleteShare.bind(shareController)
  );

  app.patch(
    "/shares/:shareId/password",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "updateSharePassword",
        summary: "Update share password",
        params: z.object({
          shareId: z.string(),
        }),
        body: UpdateSharePasswordSchema,
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.updatePassword.bind(shareController)
  );

  app.post(
    "/shares/:shareId/items",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "addItems",
        summary: "Add files and/or folders to share",
        params: z.object({
          shareId: z.string().describe("The share ID"),
        }),
        body: UpdateShareItemsSchema,
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.addItems.bind(shareController)
  );

  app.delete(
    "/shares/:shareId/items",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "removeItems",
        summary: "Remove files and/or folders from share",
        params: z.object({
          shareId: z.string().describe("The share ID"),
        }),
        body: UpdateShareItemsSchema,
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.removeItems.bind(shareController)
  );

  app.post(
    "/shares/:shareId/recipients",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "addRecipients",
        summary: "Add recipients to a share",
        params: z.object({
          shareId: z.string().describe("The share ID"),
        }),
        body: UpdateShareRecipientsSchema,
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.addRecipients.bind(shareController)
  );

  app.delete(
    "/shares/:shareId/recipients",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "removeRecipients",
        summary: "Remove recipients from a share",
        description: "Remove recipients from a share",
        params: z.object({
          shareId: z.string().describe("The share ID"),
        }),
        body: UpdateShareRecipientsSchema,
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    shareController.removeRecipients.bind(shareController)
  );

  app.post(
    "/shares/:shareId/alias",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "createShareAlias",
        summary: "Create or update share alias",
        params: z.object({
          shareId: z.string().describe("The share ID"),
        }),
        body: z.object({
          alias: z
            .string()
            .regex(/^[a-zA-Z0-9]+$/, "Alias must contain only letters and numbers")
            .min(3, "Alias must be at least 3 characters long")
            .max(30, "Alias must not exceed 30 characters"),
        }),
        response: {
          200: z.object({
            alias: ShareAliasResponseSchema,
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    shareController.createOrUpdateAlias.bind(shareController)
  );

  app.get(
    "/shares/alias/:alias",
    {
      schema: {
        tags: ["Share"],
        operationId: "getShareByAlias",
        summary: "Get share by alias",
        params: z.object({
          alias: z.string().describe("The share alias"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("The share password"),
        }),
        response: {
          200: z.object({
            share: ShareResponseSchema,
          }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    shareController.getShareByAlias.bind(shareController)
  );

  app.post(
    "/shares/:shareId/notify",
    {
      preValidation,
      schema: {
        tags: ["Share"],
        operationId: "notifyRecipients",
        summary: "Send email notification to share recipients",
        description: "Send email notification with share link to all recipients",
        params: z.object({
          shareId: z.string().describe("The share ID"),
        }),
        body: z.object({
          shareLink: z.string().url().describe("The frontend share URL"),
        }),
        response: {
          200: z.object({
            message: z.string().describe("Success message"),
            notifiedRecipients: z.array(z.string()).describe("List of notified email addresses"),
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    shareController.notifyRecipients.bind(shareController)
  );

  app.get(
    "/shares/alias/:alias/metadata",
    {
      schema: {
        tags: ["Share"],
        operationId: "getShareMetadataByAlias",
        summary: "Get share metadata by alias for Open Graph",
        description: "Get lightweight metadata for a share by alias, used for social media previews",
        params: z.object({
          alias: z.string().describe("The share alias"),
        }),
        response: {
          200: z.object({
            name: z.string().nullable(),
            description: z.string().nullable(),
            totalFiles: z.number(),
            totalFolders: z.number(),
            hasPassword: z.boolean(),
            isExpired: z.boolean(),
            isMaxViewsReached: z.boolean(),
          }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    shareController.getShareMetadataByAlias.bind(shareController)
  );
}
