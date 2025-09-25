import { FastifyInstance, FastifyReply, FastifyRequest } from "fastify";
import { z } from "zod";

import { BulkDownloadController } from "./controller";

export async function bulkDownloadRoutes(app: FastifyInstance) {
  const bulkDownloadController = new BulkDownloadController();

  const preValidation = async (request: FastifyRequest, reply: FastifyReply) => {
    try {
      await request.jwtVerify();
    } catch (err) {
      console.error(err);
      reply.status(401).send({ error: "Token inválido ou ausente." });
    }
  };

  app.post(
    "/bulk-download",
    {
      schema: {
        tags: ["Bulk Download"],
        operationId: "bulkDownloadFiles",
        summary: "Download multiple files as ZIP",
        description: "Downloads multiple files and folders as a ZIP archive",
        body: z.object({
          fileIds: z.array(z.string()).describe("Array of file IDs to download"),
          folderIds: z.array(z.string()).describe("Array of folder IDs to download"),
          zipName: z.string().describe("Name of the ZIP file"),
        }),
        response: {
          200: z.string().describe("ZIP file stream"),
          400: z.object({ error: z.string().describe("Error message") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    bulkDownloadController.downloadFiles.bind(bulkDownloadController)
  );

  app.get(
    "/bulk-download/folder/:folderId/:folderName",
    {
      schema: {
        tags: ["Bulk Download"],
        operationId: "downloadFolder",
        summary: "Download folder as ZIP",
        description: "Downloads a folder and all its files as a ZIP archive",
        params: z.object({
          folderId: z.string().describe("Folder ID"),
          folderName: z.string().describe("Folder name"),
        }),
        response: {
          200: z.string().describe("ZIP file stream"),
          404: z.object({ error: z.string().describe("Error message") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    bulkDownloadController.downloadFolder.bind(bulkDownloadController)
  );

  app.post(
    "/bulk-download/reverse-share",
    {
      preValidation,
      schema: {
        tags: ["Bulk Download"],
        operationId: "bulkDownloadReverseShareFiles",
        summary: "Download multiple reverse share files as ZIP",
        description:
          "Downloads multiple reverse share files as a ZIP archive. Only the creator of the reverse share can download files.",
        body: z.object({
          fileIds: z.array(z.string()).describe("Array of reverse share file IDs to download"),
          zipName: z.string().describe("Name of the ZIP file"),
        }),
        response: {
          200: z.string().describe("ZIP file stream"),
          400: z.object({ error: z.string().describe("Error message") }),
          401: z.object({ error: z.string().describe("Unauthorized") }),
          404: z.object({ error: z.string().describe("No files found or unauthorized") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    bulkDownloadController.downloadReverseShareFiles.bind(bulkDownloadController)
  );
}
