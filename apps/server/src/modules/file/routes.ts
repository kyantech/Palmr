import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { FileController } from "./controller";
import { CheckFileSchema, ListFilesSchema, MoveFileSchema, RegisterFileSchema, UpdateFileSchema } from "./dto";

export async function fileRoutes(app: FastifyInstance) {
  const fileController = new FileController();

  const preValidation = async (request: FastifyRequest, reply: FastifyReply) => {
    try {
      await request.jwtVerify();
    } catch (err) {
      console.error(err);
      reply.status(401).send({ error: "Token inválido ou ausente." });
    }
  };

  app.get(
    "/files/presigned-url",
    {
      preValidation,
      schema: {
        tags: ["File"],
        operationId: "getPresignedUrl",
        summary: "Get Presigned URL",
        description: "Generates a pre-signed URL for direct upload to S3-compatible storage or local filesystem",
        querystring: z.object({
          filename: z.string().min(1, "The filename is required").describe("The filename of the file"),
          extension: z.string().min(1, "The extension is required").describe("The extension of the file"),
        }),
        response: {
          200: z.object({
            url: z.string().describe("The pre-signed URL"),
            objectName: z.string().describe("The object name of the file"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    fileController.getPresignedUrl.bind(fileController)
  );

  app.post(
    "/files",
    {
      schema: {
        tags: ["File"],
        operationId: "registerFile",
        summary: "Register File Metadata",
        description: "Registers file metadata in the database",
        body: RegisterFileSchema,
        response: {
          201: z.object({
            file: z.object({
              id: z.string().describe("The file ID"),
              name: z.string().describe("The file name"),
              description: z.string().nullable().describe("The file description"),
              extension: z.string().describe("The file extension"),
              size: z.string().describe("The file size"),
              objectName: z.string().describe("The object name of the file"),
              userId: z.string().describe("The user ID"),
              folderId: z.string().nullable().describe("The folder ID"),
              createdAt: z.date().describe("The file creation date"),
              updatedAt: z.date().describe("The file last update date"),
            }),
            message: z.string().describe("The file registration message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    fileController.registerFile.bind(fileController)
  );

  app.post(
    "/files/check",
    {
      preValidation,
      schema: {
        tags: ["File"],
        operationId: "checkFile",
        summary: "Check File validity",
        description: "Checks if the file meets all requirements",
        body: CheckFileSchema,
        response: {
          201: z.object({
            message: z.string().describe("The file check success message"),
          }),
          400: z.object({
            error: z.string().describe("Error message"),
            code: z.string().optional().describe("Error code"),
            details: z.string().optional().describe("Error details"),
          }),
          401: z.object({
            error: z.string().describe("Error message"),
            code: z.string().optional().describe("Error code"),
          }),
        },
      },
    },
    fileController.checkFile.bind(fileController)
  );

  app.get(
    "/files/:objectName/download",
    {
      preValidation,
      schema: {
        tags: ["File"],
        operationId: "getDownloadUrl",
        summary: "Get Download URL",
        description: "Generates a pre-signed URL for downloading a file",
        params: z.object({
          objectName: z.string().min(1, "The objectName is required"),
        }),
        response: {
          200: z.object({
            url: z.string().describe("The download URL"),
            expiresIn: z.number().describe("The expiration time in seconds"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    fileController.getDownloadUrl.bind(fileController)
  );

  app.get(
    "/files",
    {
      preValidation,
      schema: {
        tags: ["File"],
        operationId: "listFiles",
        summary: "List Files",
        description: "Lists user files recursively by default, optionally filtered by folder",
        querystring: ListFilesSchema,
        response: {
          200: z.object({
            files: z.array(
              z.object({
                id: z.string().describe("The file ID"),
                name: z.string().describe("The file name"),
                description: z.string().nullable().describe("The file description"),
                extension: z.string().describe("The file extension"),
                size: z.string().describe("The file size"),
                objectName: z.string().describe("The object name of the file"),
                userId: z.string().describe("The user ID"),
                folderId: z.string().nullable().describe("The folder ID"),
                relativePath: z.string().nullable().describe("The relative path (only for recursive listing)"),
                createdAt: z.date().describe("The file creation date"),
                updatedAt: z.date().describe("The file last update date"),
              })
            ),
          }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    fileController.listFiles.bind(fileController)
  );

  app.patch(
    "/files/:id",
    {
      preValidation,
      schema: {
        tags: ["File"],
        operationId: "updateFile",
        summary: "Update File Metadata",
        description: "Updates file metadata in the database",
        params: z.object({
          id: z.string().min(1, "The file id is required").describe("The file ID"),
        }),
        body: UpdateFileSchema,
        response: {
          200: z.object({
            file: z.object({
              id: z.string().describe("The file ID"),
              name: z.string().describe("The file name"),
              description: z.string().nullable().describe("The file description"),
              extension: z.string().describe("The file extension"),
              size: z.string().describe("The file size"),
              objectName: z.string().describe("The object name of the file"),
              userId: z.string().describe("The user ID"),
              folderId: z.string().nullable().describe("The folder ID"),
              createdAt: z.date().describe("The file creation date"),
              updatedAt: z.date().describe("The file last update date"),
            }),
            message: z.string().describe("Success message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          403: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    fileController.updateFile.bind(fileController)
  );

  app.put(
    "/files/:id/move",
    {
      preValidation,
      schema: {
        tags: ["File"],
        operationId: "moveFile",
        summary: "Move File",
        description: "Moves a file to a different folder",
        params: z.object({
          id: z.string().min(1, "The file id is required").describe("The file ID"),
        }),
        body: MoveFileSchema,
        response: {
          200: z.object({
            file: z.object({
              id: z.string().describe("The file ID"),
              name: z.string().describe("The file name"),
              description: z.string().nullable().describe("The file description"),
              extension: z.string().describe("The file extension"),
              size: z.string().describe("The file size"),
              objectName: z.string().describe("The object name of the file"),
              userId: z.string().describe("The user ID"),
              folderId: z.string().nullable().describe("The folder ID"),
              createdAt: z.date().describe("The file creation date"),
              updatedAt: z.date().describe("The file last update date"),
            }),
            message: z.string().describe("Success message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          403: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    fileController.moveFile.bind(fileController)
  );

  app.delete(
    "/files/:id",
    {
      preValidation,
      schema: {
        tags: ["File"],
        operationId: "deleteFile",
        summary: "Delete File",
        description: "Deletes a user file",
        params: z.object({
          id: z.string().min(1, "The file id is required").describe("The file ID"),
        }),
        response: {
          200: z.object({
            message: z.string().describe("The file deletion message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    fileController.deleteFile.bind(fileController)
  );
}
