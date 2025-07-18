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

  app.get(
    "/filesystem/upload-progress/:fileId",
    {
      schema: {
        tags: ["Filesystem"],
        operationId: "getUploadProgress",
        summary: "Get chunked upload progress",
        description: "Get the progress of a chunked upload",
        params: z.object({
          fileId: z.string().describe("File ID"),
        }),
        response: {
          200: z.object({
            uploaded: z.number(),
            total: z.number(),
            percentage: z.number(),
          }),
          404: z.object({
            error: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.getUploadProgress.bind(filesystemController)
  );

  app.delete(
    "/filesystem/cancel-upload/:fileId",
    {
      schema: {
        tags: ["Filesystem"],
        operationId: "cancelUpload",
        summary: "Cancel chunked upload",
        description: "Cancel an ongoing chunked upload",
        params: z.object({
          fileId: z.string().describe("File ID"),
        }),
        response: {
          200: z.object({
            message: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.cancelUpload.bind(filesystemController)
  );

  // Resumable Download Routes
  app.get(
    "/filesystem/download-session/:sessionId",
    {
      schema: {
        tags: ["Filesystem"],
        operationId: "getDownloadSession",
        summary: "Get resumable download session info",
        description: "Get information about a resumable download session",
        params: z.object({
          sessionId: z.string().describe("Download session ID"),
        }),
        response: {
          200: z.object({
            sessionId: z.string(),
            fileName: z.string(),
            fileSize: z.number(),
            bytesDownloaded: z.number(),
            progress: z.number(),
            isActive: z.boolean(),
            createdAt: z.number(),
            expiresAt: z.number(),
            lastActivity: z.number(),
          }),
          404: z.object({
            error: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.getDownloadSession.bind(filesystemController)
  );

  app.get(
    "/filesystem/download-sessions",
    {
      schema: {
        tags: ["Filesystem"],
        operationId: "getActiveDownloadSessions",
        summary: "Get all active download sessions",
        description: "Get information about all active resumable download sessions for monitoring",
        response: {
          200: z.object({
            stats: z.object({
              totalSessions: z.number(),
              activeSessions: z.number(),
              totalBytes: z.number(),
              downloadedBytes: z.number(),
            }),
            sessions: z.array(
              z.object({
                sessionId: z.string(),
                fileName: z.string(),
                fileSize: z.number(),
                bytesDownloaded: z.number(),
                progress: z.number(),
                isActive: z.boolean(),
                createdAt: z.number(),
                lastActivity: z.number(),
              })
            ),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.getActiveDownloadSessions.bind(filesystemController)
  );

  app.get(
    "/filesystem/resume-download/:sessionId",
    {
      bodyLimit: 1024 * 1024 * 1024 * 1024 * 1024, // 1PB limit
      schema: {
        tags: ["Filesystem"],
        operationId: "resumeDownload",
        summary: "Resume a download from where it left off",
        description: "Resume a previously interrupted download using the session ID",
        params: z.object({
          sessionId: z.string().describe("Download session ID"),
        }),
        response: {
          206: z.string().describe("Partial file content"),
          403: z.object({
            error: z.string(),
          }),
          404: z.object({
            error: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.resumeDownload.bind(filesystemController)
  );

  app.delete(
    "/filesystem/download-session/:sessionId",
    {
      schema: {
        tags: ["Filesystem"],
        operationId: "cancelDownloadSession",
        summary: "Cancel a resumable download session",
        description: "Cancel and remove a resumable download session",
        params: z.object({
          sessionId: z.string().describe("Download session ID"),
        }),
        response: {
          200: z.object({
            message: z.string(),
          }),
          403: z.object({
            error: z.string(),
          }),
          404: z.object({
            error: z.string(),
          }),
          500: z.object({
            error: z.string(),
          }),
        },
      },
    },
    filesystemController.cancelDownloadSession.bind(filesystemController)
  );
}
