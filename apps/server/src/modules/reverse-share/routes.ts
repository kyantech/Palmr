import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { ReverseShareController } from "./controller";
import {
  CreateReverseShareSchema,
  GetPresignedUrlSchema,
  ReverseShareFileSchema,
  ReverseSharePasswordSchema,
  ReverseSharePublicSchema,
  ReverseShareResponseSchema,
  UpdateReverseShareFileSchema,
  UpdateReverseSharePasswordSchema,
  UpdateReverseShareSchema,
  UploadToReverseShareSchema,
} from "./dto";

export async function reverseShareRoutes(app: FastifyInstance) {
  const reverseShareController = new ReverseShareController();

  const preValidation = async (request: FastifyRequest, reply: FastifyReply) => {
    try {
      await request.jwtVerify();
    } catch (err) {
      console.error(err);
      reply.status(401).send({ error: "Token inválido ou ausente." });
    }
  };

  app.post(
    "/reverse-shares",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "createReverseShare",
        summary: "Create Reverse Share",
        description:
          "Create a new reverse share to allow others to upload files to you. Only authenticated users can create reverse shares. The reverse share can be configured with various restrictions like file count limits, file size limits, allowed file types, password protection, and expiration dates.",
        body: CreateReverseShareSchema,
        response: {
          201: z.object({
            reverseShare: ReverseShareResponseSchema,
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.createReverseShare.bind(reverseShareController)
  );

  app.get(
    "/reverse-shares",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "listUserReverseShares",
        summary: "List User's Reverse Shares",
        description:
          "Retrieve all reverse shares created by the authenticated user, ordered by creation date (newest first). This endpoint returns comprehensive information about each reverse share including file counts and settings.",
        response: {
          200: z.object({
            reverseShares: z.array(ReverseShareResponseSchema),
          }),
          401: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.listUserReverseShares.bind(reverseShareController)
  );

  app.get(
    "/reverse-shares/:id",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "getReverseShare",
        summary: "Get Reverse Share Details",
        description:
          "Retrieve detailed information about a specific reverse share by its ID. Only the creator of the reverse share can access this endpoint. Returns all configuration details and uploaded files.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share"),
        }),
        response: {
          200: z.object({
            reverseShare: ReverseShareResponseSchema,
          }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.getReverseShare.bind(reverseShareController)
  );

  app.put(
    "/reverse-shares",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "updateReverseShare",
        summary: "Update Reverse Share",
        description:
          "Update the configuration of an existing reverse share. Only the creator can update their reverse share. All fields except 'id' are optional - only provided fields will be updated.",
        body: UpdateReverseShareSchema,
        response: {
          200: z.object({
            reverseShare: ReverseShareResponseSchema,
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.updateReverseShare.bind(reverseShareController)
  );

  app.put(
    "/reverse-shares/:id/password",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "updateReverseSharePassword",
        summary: "Update Reverse Share Password",
        description:
          "Update or remove the password for a reverse share. Send null as password value to remove password protection. Only the creator can update the password.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share"),
        }),
        body: UpdateReverseSharePasswordSchema,
        response: {
          200: z.object({
            reverseShare: ReverseShareResponseSchema,
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.updatePassword.bind(reverseShareController)
  );

  app.delete(
    "/reverse-shares/:id",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "deleteReverseShare",
        summary: "Delete Reverse Share",
        description:
          "Delete a reverse share and all its associated files. Only the creator of the reverse share can delete it. This action is irreversible and will permanently remove all uploaded files.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share to delete"),
        }),
        response: {
          200: z.object({
            reverseShare: ReverseShareResponseSchema,
          }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.deleteReverseShare.bind(reverseShareController)
  );

  app.get(
    "/reverse-shares/:id/upload",
    {
      schema: {
        tags: ["Reverse Share"],
        operationId: "getReverseShareForUpload",
        summary: "Get Reverse Share for Upload (Public)",
        description:
          "Get reverse share information for file upload. This is a public endpoint that allows anyone with the link to view upload requirements and restrictions. If password protected, provide password as query parameter.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("Password for accessing password-protected reverse shares"),
        }),
        response: {
          200: z.object({
            reverseShare: ReverseSharePublicSchema,
          }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
          410: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.getReverseShareForUpload.bind(reverseShareController)
  );

  app.get(
    "/reverse-shares/alias/:alias/upload",
    {
      schema: {
        tags: ["Reverse Share"],
        operationId: "getReverseShareForUploadByAlias",
        summary: "Get Reverse Share for Upload by Alias (Public)",
        description:
          "Get reverse share information for file upload using alias. This is a public endpoint that allows anyone with the alias to view upload requirements and restrictions. If password protected, provide password as query parameter.",
        params: z.object({
          alias: z.string().describe("Alias of the reverse share"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("Password for accessing password-protected reverse shares"),
        }),
        response: {
          200: z.object({
            reverseShare: ReverseSharePublicSchema,
          }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
          410: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.getReverseShareForUploadByAlias.bind(reverseShareController)
  );

  app.post(
    "/reverse-shares/:id/presigned-url",
    {
      schema: {
        tags: ["Reverse Share"],
        operationId: "getPresignedUrl",
        summary: "Get Presigned URL for File Upload (Public)",
        description:
          "Get a presigned URL for direct file upload to storage. This endpoint validates reverse share permissions and generates a temporary upload URL. The presigned URL allows clients to upload files directly to the storage service without going through the API server.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("Password for accessing password-protected reverse shares"),
        }),
        body: GetPresignedUrlSchema,
        response: {
          200: z.object({
            url: z.string().describe("Presigned URL for file upload"),
            expiresIn: z.number().describe("URL expiration time in seconds"),
          }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
          410: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.getPresignedUrl.bind(reverseShareController)
  );

  app.post(
    "/reverse-shares/alias/:alias/presigned-url",
    {
      schema: {
        tags: ["Reverse Share"],
        operationId: "getPresignedUrlByAlias",
        summary: "Get Presigned URL for File Upload by Alias (Public)",
        description:
          "Get a presigned URL for direct file upload to storage using alias. This endpoint validates reverse share permissions and generates a temporary upload URL. The presigned URL allows clients to upload files directly to the storage service without going through the API server.",
        params: z.object({
          alias: z.string().describe("Alias of the reverse share"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("Password for accessing password-protected reverse shares"),
        }),
        body: GetPresignedUrlSchema,
        response: {
          200: z.object({
            url: z.string().describe("Presigned URL for file upload"),
            expiresIn: z.number().describe("URL expiration time in seconds"),
          }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
          410: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.getPresignedUrlByAlias.bind(reverseShareController)
  );

  app.post(
    "/reverse-shares/:id/register-file",
    {
      schema: {
        tags: ["Reverse Share"],
        operationId: "registerFileUpload",
        summary: "Register File Upload Completion (Public)",
        description:
          "Register a completed file upload to the reverse share. This endpoint should be called after successfully uploading a file using the presigned URL to record the file metadata and associate it with the reverse share.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("Password for accessing password-protected reverse shares"),
        }),
        body: UploadToReverseShareSchema,
        response: {
          201: z.object({
            file: ReverseShareFileSchema,
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
          410: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.registerFileUpload.bind(reverseShareController)
  );

  app.post(
    "/reverse-shares/alias/:alias/register-file",
    {
      schema: {
        tags: ["Reverse Share"],
        operationId: "registerFileUploadByAlias",
        summary: "Register File Upload Completion by Alias (Public)",
        description:
          "Register a completed file upload to the reverse share using alias. This endpoint should be called after successfully uploading a file using the presigned URL to record the file metadata and associate it with the reverse share.",
        params: z.object({
          alias: z.string().describe("Alias of the reverse share"),
        }),
        querystring: z.object({
          password: z.string().optional().describe("Password for accessing password-protected reverse shares"),
        }),
        body: UploadToReverseShareSchema,
        response: {
          201: z.object({
            file: ReverseShareFileSchema,
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
          410: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.registerFileUploadByAlias.bind(reverseShareController)
  );

  app.post(
    "/reverse-shares/:id/check-password",
    {
      schema: {
        tags: ["Reverse Share"],
        operationId: "checkReverseSharePassword",
        summary: "Verify Reverse Share Password (Public)",
        description:
          "Verify if the provided password is correct for a password-protected reverse share. This endpoint allows frontend applications to validate passwords before attempting uploads. Returns whether the password is valid.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share"),
        }),
        body: ReverseSharePasswordSchema,
        response: {
          200: z.object({
            valid: z.boolean().describe("Whether the provided password is valid"),
          }),
          401: z.object({
            error: z.string(),
            valid: z.boolean(),
          }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.checkPassword.bind(reverseShareController)
  );

  app.get(
    "/reverse-shares/files/:fileId/download",
    {
      preValidation,
      bodyLimit: 1024 * 1024 * 1024 * 1024 * 1024, // 1PB limit for large video files
      schema: {
        tags: ["Reverse Share"],
        operationId: "downloadReverseShareFile",
        summary: "Download File from Reverse Share",
        description:
          "Generate a download URL for a file uploaded to a reverse share. Only the creator of the reverse share can download files. The URL expires after 1 hour and works with both S3 and filesystem storage modes.",
        params: z.object({
          fileId: z.string().describe("Unique identifier of the file to download"),
        }),
        response: {
          200: z.object({
            url: z.string().describe("Presigned download URL - expires after 1 hour"),
            expiresIn: z.number().describe("URL expiration time in seconds (3600 = 1 hour)"),
          }),
          202: z.object({
            queued: z.boolean().describe("Download was queued due to memory constraints"),
            downloadId: z.string().describe("Download identifier for tracking"),
            message: z.string().describe("Queue status message"),
            estimatedWaitTime: z.number().describe("Estimated wait time in seconds"),
          }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.downloadFile.bind(reverseShareController)
  );

  app.delete(
    "/reverse-shares/files/:fileId",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "deleteReverseShareFile",
        summary: "Delete File from Reverse Share",
        description:
          "Permanently delete a file from a reverse share. The file will be removed from both the database and storage. This action cannot be undone. Only the creator of the reverse share can delete files.",
        params: z.object({
          fileId: z.string().describe("Unique identifier of the file to delete"),
        }),
        response: {
          200: z.object({
            file: ReverseShareFileSchema,
          }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.deleteFile.bind(reverseShareController)
  );

  app.post(
    "/reverse-shares/:reverseShareId/alias",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "createReverseShareAlias",
        summary: "Create or update reverse share alias",
        description: "Create or update a custom alias for a reverse share to make it easier to share and remember.",
        params: z.object({
          reverseShareId: z.string().describe("The reverse share ID"),
        }),
        body: z.object({
          alias: z
            .string()
            .regex(/^[a-zA-Z0-9-]+$/, "Alias must contain only letters, numbers, and hyphens")
            .min(3, "Alias must be at least 3 characters long")
            .max(30, "Alias must not exceed 30 characters"),
        }),
        response: {
          200: z.object({
            alias: z.object({
              id: z.string(),
              alias: z.string(),
              reverseShareId: z.string(),
              createdAt: z.string(),
              updatedAt: z.string(),
            }),
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.createOrUpdateAlias.bind(reverseShareController)
  );

  app.patch(
    "/reverse-shares/:id/activate",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "activateReverseShare",
        summary: "Activate Reverse Share",
        description:
          "Activate a reverse share to make it available for uploads. Only the creator can activate their reverse share.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share to activate"),
        }),
        response: {
          200: z.object({
            reverseShare: ReverseShareResponseSchema,
          }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.activateReverseShare.bind(reverseShareController)
  );

  app.patch(
    "/reverse-shares/:id/deactivate",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "deactivateReverseShare",
        summary: "Deactivate Reverse Share",
        description:
          "Deactivate a reverse share to prevent new uploads. Only the creator can deactivate their reverse share.",
        params: z.object({
          id: z.string().describe("Unique identifier of the reverse share to deactivate"),
        }),
        response: {
          200: z.object({
            reverseShare: ReverseShareResponseSchema,
          }),
          401: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.deactivateReverseShare.bind(reverseShareController)
  );

  app.put(
    "/reverse-shares/files/:fileId",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "updateReverseShareFile",
        summary: "Update File from Reverse Share",
        description:
          "Update the name and/or description of a file uploaded to a reverse share. Only the creator of the reverse share can update files.",
        params: z.object({
          fileId: z.string().describe("Unique identifier of the file to update"),
        }),
        body: UpdateReverseShareFileSchema,
        response: {
          200: z.object({
            file: ReverseShareFileSchema,
          }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.updateFile.bind(reverseShareController)
  );

  app.post(
    "/reverse-shares/files/:fileId/copy",
    {
      preValidation,
      schema: {
        tags: ["Reverse Share"],
        operationId: "copyReverseShareFileToUserFiles",
        summary: "Copy File from Reverse Share to User Files",
        description:
          "Copy a file from a reverse share to the user's personal files. Only the creator of the reverse share can copy files. The file will be duplicated in storage and added to the user's file collection.",
        params: z.object({
          fileId: z.string().describe("Unique identifier of the file to copy"),
        }),
        response: {
          200: z.object({
            file: z.object({
              id: z.string(),
              name: z.string(),
              description: z.string().nullable(),
              extension: z.string(),
              size: z.string(),
              objectName: z.string(),
              userId: z.string(),
              createdAt: z.string(),
              updatedAt: z.string(),
            }),
            message: z.string(),
          }),
          400: z.object({ error: z.string() }),
          401: z.object({ error: z.string() }),
          403: z.object({ error: z.string() }),
          404: z.object({ error: z.string() }),
        },
      },
    },
    reverseShareController.copyFileToUserFiles.bind(reverseShareController)
  );
}
