import { FastifyInstance } from "fastify";
import { z } from "zod";

import { FilesystemController } from "./controller";

export async function downloadQueueRoutes(app: FastifyInstance) {
  const filesystemController = new FilesystemController();

  app.get(
    "/filesystem/download-queue/status",
    {
      schema: {
        tags: ["Download Queue"],
        operationId: "getDownloadQueueStatus",
        summary: "Get download queue status",
        description: "Get current status of the download queue including active downloads and queue length",
        response: {
          200: z.object({
            status: z.string(),
            data: z.object({
              queueLength: z.number(),
              maxQueueSize: z.number(),
              activeDownloads: z.number(),
              maxConcurrent: z.number(),
              queuedDownloads: z.array(
                z.object({
                  downloadId: z.string(),
                  position: z.number(),
                  waitTime: z.number(),
                  fileName: z.string().optional(),
                  fileSize: z.number().optional(),
                })
              ),
            }),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.getQueueStatus.bind(filesystemController)
  );

  app.delete(
    "/filesystem/download-queue/:downloadId",
    {
      schema: {
        tags: ["Download Queue"],
        operationId: "cancelQueuedDownload",
        summary: "Cancel a queued download",
        description: "Cancel a specific download that is waiting in the queue",
        params: z.object({
          downloadId: z.string().describe("Download ID"),
        }),
        response: {
          200: z.object({
            message: z.string(),
            downloadId: z.string(),
          }),
          404: z.object({
            error: z.string(),
            downloadId: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.cancelQueuedDownload.bind(filesystemController)
  );

  app.delete(
    "/filesystem/download-queue",
    {
      schema: {
        tags: ["Download Queue"],
        operationId: "clearDownloadQueue",
        summary: "Clear entire download queue",
        description: "Cancel all downloads waiting in the queue (admin operation)",
        response: {
          200: z.object({
            message: z.string(),
            clearedCount: z.number(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.clearDownloadQueue.bind(filesystemController)
  );
}
