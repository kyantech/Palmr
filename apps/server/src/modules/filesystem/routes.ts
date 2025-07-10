import { FastifyInstance, FastifyRequest } from "fastify";
import { z } from "zod";

import { FilesystemController } from "./controller";

export async function filesystemRoutes(app: FastifyInstance) {
  const filesystemController = new FilesystemController();

  app.addContentTypeParser("*", async (request: FastifyRequest, payload: any) => {
    return payload;
  });

  app.addContentTypeParser("application/json", async (request: FastifyRequest, payload: any) => {
    return payload;
  });

  app.put(
    "/filesystem/upload/:token",
    {
      bodyLimit: 1024 * 1024 * 1024 * 1024 * 1024, // 1PB limit
      schema: {
        tags: ["Filesystem"],
        operationId: "uploadToFilesystem",
        summary: "Upload file to filesystem storage",
        description: "Upload a file directly to the encrypted filesystem storage",
        params: z.object({
          token: z.string().describe("Upload token"),
        }),
        response: {
          200: z.object({
            message: z.string(),
          }),
          400: z.object({
            error: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.upload.bind(filesystemController)
  );

  app.get(
    "/filesystem/download/:token",
    {
      bodyLimit: 1024 * 1024 * 1024 * 1024 * 1024, // 1PB limit
      schema: {
        tags: ["Filesystem"],
        operationId: "downloadFromFilesystem",
        summary: "Download file from filesystem storage",
        description: "Download a file directly from the encrypted filesystem storage",
        params: z.object({
          token: z.string().describe("Download token"),
        }),
        response: {
          200: z.string().describe("File content"),
          400: z.object({
            error: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.download.bind(filesystemController)
  );
}
