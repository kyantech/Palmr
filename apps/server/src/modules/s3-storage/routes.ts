/**
 * S3 Storage Routes
 *
 * Simple routes for S3-based storage using presigned URLs.
 * Much simpler than filesystem routes - no chunk management, no streaming.
 */

import { FastifyInstance } from "fastify";
import { z } from "zod";

import { S3StorageController } from "./controller";

export async function s3StorageRoutes(app: FastifyInstance) {
  const controller = new S3StorageController();

  // Get presigned upload URL
  app.post(
    "/s3/upload-url",
    {
      schema: {
        tags: ["S3 Storage"],
        operationId: "getS3UploadUrl",
        summary: "Get presigned URL for upload",
        description: "Returns a presigned URL that clients can use to upload directly to S3",
        body: z.object({
          objectName: z.string().describe("Object name/path in S3"),
          expires: z.number().optional().describe("URL expiration in seconds (default: 3600)"),
        }),
        response: {
          200: z.object({
            uploadUrl: z.string(),
            objectName: z.string(),
            expiresIn: z.number(),
            message: z.string(),
          }),
        },
      },
    },
    controller.getUploadUrl.bind(controller)
  );

  // Get presigned download URL
  app.get(
    "/s3/download-url",
    {
      schema: {
        tags: ["S3 Storage"],
        operationId: "getS3DownloadUrl",
        summary: "Get presigned URL for download",
        description: "Returns a presigned URL that clients can use to download directly from S3",
        querystring: z.object({
          objectName: z.string().describe("Object name/path in S3"),
          expires: z.string().optional().describe("URL expiration in seconds (default: 3600)"),
          fileName: z.string().optional().describe("Optional filename for download"),
        }),
        response: {
          200: z.object({
            downloadUrl: z.string(),
            objectName: z.string(),
            expiresIn: z.number(),
            message: z.string(),
          }),
        },
      },
    },
    controller.getDownloadUrl.bind(controller)
  );

  // Delete object
  app.delete(
    "/s3/object/:objectName",
    {
      schema: {
        tags: ["S3 Storage"],
        operationId: "deleteS3Object",
        summary: "Delete object from S3",
        params: z.object({
          objectName: z.string().describe("Object name/path in S3"),
        }),
        response: {
          200: z.object({
            message: z.string(),
            objectName: z.string(),
          }),
        },
      },
    },
    controller.deleteObject.bind(controller)
  );

  // Check if object exists
  app.get(
    "/s3/exists",
    {
      schema: {
        tags: ["S3 Storage"],
        operationId: "checkS3ObjectExists",
        summary: "Check if object exists in S3",
        querystring: z.object({
          objectName: z.string().describe("Object name/path in S3"),
        }),
        response: {
          200: z.object({
            exists: z.boolean(),
            objectName: z.string(),
          }),
        },
      },
    },
    controller.checkExists.bind(controller)
  );
}
