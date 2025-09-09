import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { FolderController } from "./controller";
import {
  CheckFolderSchema,
  FolderResponseSchema,
  ListFoldersSchema,
  MoveFolderSchema,
  RegisterFolderSchema,
  UpdateFolderSchema,
} from "./dto";

export async function folderRoutes(app: FastifyInstance) {
  const folderController = new FolderController();

  const preValidation = async (request: FastifyRequest, reply: FastifyReply) => {
    try {
      await request.jwtVerify();
    } catch (err) {
      console.error(err);
      reply.status(401).send({ error: "Token inválido ou ausente." });
    }
  };

  app.post(
    "/folders",
    {
      schema: {
        tags: ["Folder"],
        operationId: "registerFolder",
        summary: "Register Folder Metadata",
        description: "Registers folder metadata in the database",
        body: RegisterFolderSchema,
        response: {
          201: z.object({
            folder: z.object({
              id: z.string().describe("The folder ID"),
              name: z.string().describe("The folder name"),
              description: z.string().nullable().describe("The folder description"),
              parentId: z.string().nullable().describe("The parent folder ID"),
              userId: z.string().describe("The user ID"),
              createdAt: z.date().describe("The folder creation date"),
              updatedAt: z.date().describe("The folder last update date"),
              totalSize: z.string().optional().describe("The total size of the folder"),
              _count: z
                .object({
                  files: z.number().describe("Number of files in folder"),
                  children: z.number().describe("Number of subfolders"),
                })
                .optional()
                .describe("Count statistics"),
            }),
            message: z.string().describe("The folder registration message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    folderController.registerFolder.bind(folderController)
  );

  app.post(
    "/folders/check",
    {
      preValidation,
      schema: {
        tags: ["Folder"],
        operationId: "checkFolder",
        summary: "Check Folder validity",
        description: "Checks if the folder meets all requirements",
        body: CheckFolderSchema,
        response: {
          201: z.object({
            message: z.string().describe("The folder check success message"),
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
    folderController.checkFolder.bind(folderController)
  );

  app.get(
    "/folders",
    {
      preValidation,
      schema: {
        tags: ["Folder"],
        operationId: "listFolders",
        summary: "List Folders",
        description: "Lists user folders recursively by default, optionally filtered by folder",
        querystring: ListFoldersSchema,
        response: {
          200: z.object({
            folders: z.array(
              z.object({
                id: z.string().describe("The folder ID"),
                name: z.string().describe("The folder name"),
                description: z.string().nullable().describe("The folder description"),
                parentId: z.string().nullable().describe("The parent folder ID"),
                userId: z.string().describe("The user ID"),
                createdAt: z.date().describe("The folder creation date"),
                updatedAt: z.date().describe("The folder last update date"),
                totalSize: z.string().optional().describe("The total size of the folder"),
                _count: z
                  .object({
                    files: z.number().describe("Number of files in folder"),
                    children: z.number().describe("Number of subfolders"),
                  })
                  .optional()
                  .describe("Count statistics"),
              })
            ),
          }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    folderController.listFolders.bind(folderController)
  );

  app.patch(
    "/folders/:id",
    {
      preValidation,
      schema: {
        tags: ["Folder"],
        operationId: "updateFolder",
        summary: "Update Folder Metadata",
        description: "Updates folder metadata in the database",
        params: z.object({
          id: z.string().min(1, "The folder id is required").describe("The folder ID"),
        }),
        body: UpdateFolderSchema,
        response: {
          200: z.object({
            folder: z.object({
              id: z.string().describe("The folder ID"),
              name: z.string().describe("The folder name"),
              description: z.string().nullable().describe("The folder description"),
              parentId: z.string().nullable().describe("The parent folder ID"),
              userId: z.string().describe("The user ID"),
              createdAt: z.date().describe("The folder creation date"),
              updatedAt: z.date().describe("The folder last update date"),
              totalSize: z.string().optional().describe("The total size of the folder"),
              _count: z
                .object({
                  files: z.number().describe("Number of files in folder"),
                  children: z.number().describe("Number of subfolders"),
                })
                .optional()
                .describe("Count statistics"),
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
    folderController.updateFolder.bind(folderController)
  );

  app.put(
    "/folders/:id/move",
    {
      preValidation,
      schema: {
        tags: ["Folder"],
        operationId: "moveFolder",
        summary: "Move Folder",
        description: "Moves a folder to a different parent folder",
        params: z.object({
          id: z.string().min(1, "The folder id is required").describe("The folder ID"),
        }),
        body: MoveFolderSchema,
        response: {
          200: z.object({
            folder: z.object({
              id: z.string().describe("The folder ID"),
              name: z.string().describe("The folder name"),
              description: z.string().nullable().describe("The folder description"),
              parentId: z.string().nullable().describe("The parent folder ID"),
              userId: z.string().describe("The user ID"),
              createdAt: z.date().describe("The folder creation date"),
              updatedAt: z.date().describe("The folder last update date"),
              totalSize: z.string().optional().describe("The total size of the folder"),
              _count: z
                .object({
                  files: z.number().describe("Number of files in folder"),
                  children: z.number().describe("Number of subfolders"),
                })
                .optional()
                .describe("Count statistics"),
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
    folderController.moveFolder.bind(folderController)
  );

  app.delete(
    "/folders/:id",
    {
      preValidation,
      schema: {
        tags: ["Folder"],
        operationId: "deleteFolder",
        summary: "Delete Folder",
        description: "Deletes a folder and all its contents",
        params: z.object({
          id: z.string().min(1, "The folder id is required").describe("The folder ID"),
        }),
        response: {
          200: z.object({
            message: z.string().describe("The folder deletion message"),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Error message") }),
          404: z.object({ error: z.string().describe("Error message") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    folderController.deleteFolder.bind(folderController)
  );
}
