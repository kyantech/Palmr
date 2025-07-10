import { FastifyInstance } from "fastify";
import { z } from "zod";

import { StorageController } from "./controller";

export async function storageRoutes(app: FastifyInstance) {
  const storageController = new StorageController();

  app.get(
    "/storage/disk-space",
    {
      schema: {
        tags: ["Storage"],
        operationId: "getDiskSpace",
        summary: "Get server disk space information",
        description: "Get server disk space information",
        response: {
          200: z.object({
            diskSizeGB: z.number().describe("The server disk size in GB"),
            diskUsedGB: z.number().describe("The server disk used in GB"),
            diskAvailableGB: z.number().describe("The server disk available in GB"),
            uploadAllowed: z.boolean().describe("Whether file upload is allowed"),
          }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    storageController.getDiskSpace.bind(storageController)
  );

  app.get(
    "/storage/check-upload",
    {
      schema: {
        tags: ["Storage"],
        operationId: "checkUploadAllowed",
        summary: "Check if file upload is allowed",
        description: "Check if file upload is allowed based on available space (fileSize in bytes)",
        querystring: z.object({
          fileSize: z.string().describe("The file size in bytes"),
        }),
        response: {
          200: z.object({
            diskSizeGB: z.number().describe("The server disk size in GB"),
            diskUsedGB: z.number().describe("The server disk used in GB"),
            diskAvailableGB: z.number().describe("The server disk available in GB"),
            uploadAllowed: z.boolean().describe("Whether file upload is allowed"),
            fileSizeInfo: z.object({
              bytes: z.number(),
              kb: z.number(),
              mb: z.number(),
              gb: z.number(),
            }),
          }),
          400: z.object({ error: z.string().describe("Error message") }),
          500: z.object({ error: z.string().describe("Error message") }),
        },
      },
    },
    storageController.checkUploadAllowed.bind(storageController)
  );
}
