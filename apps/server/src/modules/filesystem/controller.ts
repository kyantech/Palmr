import * as fs from "fs";
import { pipeline } from "stream/promises";
import { FastifyReply, FastifyRequest } from "fastify";

import { FilesystemStorageProvider } from "../../providers/filesystem-storage.provider";
import { DownloadCancelResponse, QueueClearResponse, QueueStatusResponse } from "../../types/download-queue";
import { DownloadMemoryManager } from "../../utils/download-memory-manager";
import { getContentType } from "../../utils/mime-types";
import { ChunkManager, ChunkMetadata } from "./chunk-manager";

export class FilesystemController {
  private chunkManager = ChunkManager.getInstance();
  private memoryManager = DownloadMemoryManager.getInstance();

  /**
   * Check if a character is valid in an HTTP token (RFC 2616)
   * Tokens can contain: alphanumeric and !#$%&'*+-.^_`|~
   * Must exclude separators: ()<>@,;:\"/[]?={} and space/tab
   */
  private isTokenChar(char: string): boolean {
    const code = char.charCodeAt(0);
    // Basic ASCII range check
    if (code < 33 || code > 126) return false;
    // Exclude separator characters per RFC 2616
    const separators = '()<>@,;:\\"/[]?={} \t';
    return !separators.includes(char);
  }

  private encodeFilenameForHeader(filename: string): string {
    if (!filename || filename.trim() === "") {
      return 'attachment; filename="download"';
    }

    let sanitized = filename
      .replace(/"/g, "'")
      .replace(/[\r\n\t\v\f]/g, "")
      .replace(/[\\|/]/g, "-")
      .replace(/[<>:|*?]/g, "");

    sanitized = sanitized
      .split("")
      .filter((char) => {
        const code = char.charCodeAt(0);
        return code >= 32 && !(code >= 127 && code <= 159);
      })
      .join("")
      .trim();

    if (!sanitized) {
      return 'attachment; filename="download"';
    }

    // Create ASCII-safe version with only valid token characters
    const asciiSafe = sanitized
      .split("")
      .filter((char) => this.isTokenChar(char))
      .join("");

    if (asciiSafe && asciiSafe.trim()) {
      const encoded = encodeURIComponent(sanitized);
      return `attachment; filename="${asciiSafe}"; filename*=UTF-8''${encoded}`;
    } else {
      const encoded = encodeURIComponent(sanitized);
      return `attachment; filename*=UTF-8''${encoded}`;
    }
  }

  async upload(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { token } = request.params as { token: string };

      const provider = FilesystemStorageProvider.getInstance();

      const tokenData = provider.validateUploadToken(token);

      if (!tokenData) {
        return reply.status(400).send({ error: "Invalid or expired upload token" });
      }

      const chunkMetadata = this.extractChunkMetadata(request);

      if (chunkMetadata) {
        try {
          const result = await this.handleChunkedUpload(request, chunkMetadata, tokenData.objectName);

          if (result.isComplete) {
            provider.consumeUploadToken(token);
            reply.status(200).send({
              message: "File uploaded successfully",
              objectName: result.finalPath,
              finalObjectName: result.finalPath,
            });
          } else {
            reply.status(200).send({
              message: "Chunk uploaded successfully",
              progress: this.chunkManager.getUploadProgress(chunkMetadata.fileId),
            });
          }
        } catch (chunkError: any) {
          return reply.status(400).send({
            error: chunkError.message || "Chunked upload failed",
            details: chunkError.toString(),
          });
        }
      } else {
        await this.uploadFileStream(request, provider, tokenData.objectName);
        provider.consumeUploadToken(token);
        reply.status(200).send({ message: "File uploaded successfully" });
      }
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  private async uploadFileStream(request: FastifyRequest, provider: FilesystemStorageProvider, objectName: string) {
    await provider.uploadFileFromStream(objectName, request.raw);
  }

  private extractChunkMetadata(request: FastifyRequest): ChunkMetadata | null {
    const fileId = request.headers["x-file-id"] as string;
    const chunkIndex = request.headers["x-chunk-index"] as string;
    const totalChunks = request.headers["x-total-chunks"] as string;
    const chunkSize = request.headers["x-chunk-size"] as string;
    const totalSize = request.headers["x-total-size"] as string;
    const encodedFileName = request.headers["x-file-name"] as string;
    const isLastChunk = request.headers["x-is-last-chunk"] as string;

    if (!fileId || !chunkIndex || !totalChunks || !chunkSize || !totalSize || !encodedFileName) {
      return null;
    }

    // Decode the base64-encoded filename to handle UTF-8 characters
    let fileName: string;
    try {
      fileName = decodeURIComponent(escape(Buffer.from(encodedFileName, "base64").toString("binary")));
    } catch (error) {
      // Fallback to the encoded value if decoding fails (for backward compatibility)
      fileName = encodedFileName;
    }

    const metadata = {
      fileId,
      chunkIndex: parseInt(chunkIndex, 10),
      totalChunks: parseInt(totalChunks, 10),
      chunkSize: parseInt(chunkSize, 10),
      totalSize: parseInt(totalSize, 10),
      fileName,
      isLastChunk: isLastChunk === "true",
    };

    return metadata;
  }

  private async handleChunkedUpload(request: FastifyRequest, metadata: ChunkMetadata, originalObjectName: string) {
    const stream = request.raw;

    stream.on("error", (error) => {
      console.error("Request stream error:", error);
    });

    return await this.chunkManager.processChunk(metadata, stream, originalObjectName);
  }

  async getUploadProgress(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { fileId } = request.params as { fileId: string };

      const progress = this.chunkManager.getUploadProgress(fileId);

      if (!progress) {
        return reply.status(404).send({ error: "Upload not found" });
      }

      reply.status(200).send(progress);
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  async cancelUpload(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { fileId } = request.params as { fileId: string };

      await this.chunkManager.cancelUpload(fileId);

      reply.status(200).send({ message: "Upload cancelled successfully" });
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  async download(request: FastifyRequest, reply: FastifyReply) {
    const downloadId = `${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;

    try {
      const { token } = request.params as { token: string };
      const provider = FilesystemStorageProvider.getInstance();

      const tokenData = provider.validateDownloadToken(token);

      if (!tokenData) {
        return reply.status(400).send({ error: "Invalid or expired download token" });
      }

      const filePath = provider.getFilePath(tokenData.objectName);

      const fileExists = await provider.fileExists(tokenData.objectName);
      if (!fileExists) {
        console.error(`[DOWNLOAD] File not found: ${tokenData.objectName}`);
        return reply.status(404).send({
          error: "File not found",
          message:
            "The requested file does not exist on the server. It may have been deleted or the upload was incomplete.",
        });
      }

      const stats = await fs.promises.stat(filePath);
      const fileSize = stats.size;
      const fileName = tokenData.fileName || "download";

      const fileSizeMB = fileSize / (1024 * 1024);
      console.log(`[DOWNLOAD] Requesting slot for ${downloadId}: ${tokenData.objectName} (${fileSizeMB.toFixed(1)}MB)`);

      try {
        await this.memoryManager.requestDownloadSlot(downloadId, {
          fileName,
          fileSize,
          objectName: tokenData.objectName,
        });
      } catch (error: any) {
        console.warn(`[DOWNLOAD] Queue full for ${downloadId}: ${error.message}`);
        return reply.status(503).send({
          error: "Download queue is full",
          message: error.message,
          retryAfter: 60,
        });
      }

      console.log(`[DOWNLOAD] Starting ${downloadId}: ${tokenData.objectName} (${fileSizeMB.toFixed(1)}MB)`);
      this.memoryManager.startDownload(downloadId);

      const range = request.headers.range;

      reply.header("Content-Disposition", this.encodeFilenameForHeader(fileName));
      reply.header("Content-Type", getContentType(fileName));
      reply.header("Accept-Ranges", "bytes");
      reply.header("X-Download-ID", downloadId);

      reply.raw.on("close", () => {
        this.memoryManager.endDownload(downloadId);
        console.log(`[DOWNLOAD] Client disconnected: ${downloadId}`);
      });

      reply.raw.on("error", () => {
        this.memoryManager.endDownload(downloadId);
        console.log(`[DOWNLOAD] Client error: ${downloadId}`);
      });

      try {
        if (range) {
          const parts = range.replace(/bytes=/, "").split("-");
          const start = parseInt(parts[0], 10);
          const end = parts[1] ? parseInt(parts[1], 10) : fileSize - 1;

          reply.status(206);
          reply.header("Content-Range", `bytes ${start}-${end}/${fileSize}`);
          reply.header("Content-Length", end - start + 1);

          await this.downloadFileRange(reply, provider, tokenData.objectName, start, end, downloadId);
        } else {
          reply.header("Content-Length", fileSize);
          await this.downloadFileStream(reply, provider, tokenData.objectName, downloadId);
        }

        provider.consumeDownloadToken(token);
      } finally {
        this.memoryManager.endDownload(downloadId);
      }
    } catch (error) {
      this.memoryManager.endDownload(downloadId);
      console.error(`[DOWNLOAD] Error in ${downloadId}:`, error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  private async downloadFileStream(
    reply: FastifyReply,
    provider: FilesystemStorageProvider,
    objectName: string,
    downloadId?: string
  ) {
    try {
      FilesystemStorageProvider.logMemoryUsage(`Download start: ${objectName} (${downloadId})`);

      const downloadStream = provider.createDownloadStream(objectName);

      downloadStream.on("error", (error) => {
        console.error("Download stream error:", error);
        FilesystemStorageProvider.logMemoryUsage(`Download error: ${objectName} (${downloadId})`);
        if (!reply.sent) {
          reply.status(500).send({ error: "Download failed" });
        }
      });

      reply.raw.on("close", () => {
        if (downloadStream.readable && typeof (downloadStream as any).destroy === "function") {
          (downloadStream as any).destroy();
        }
        FilesystemStorageProvider.logMemoryUsage(`Download client disconnect: ${objectName} (${downloadId})`);
      });

      if (this.memoryManager.shouldThrottleStream()) {
        console.log(
          `[MEMORY THROTTLE] ${objectName} - Pausing stream due to high memory usage: ${this.memoryManager.getCurrentMemoryUsageMB().toFixed(0)}MB`
        );

        const { Transform } = require("stream");
        const memoryManager = this.memoryManager;
        const throttleStream = new Transform({
          highWaterMark: 256 * 1024,
          transform(chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error | null, data?: any) => void) {
            if (memoryManager.shouldThrottleStream()) {
              setImmediate(() => {
                this.push(chunk);
                callback();
              });
            } else {
              this.push(chunk);
              callback();
            }
          },
        });

        await pipeline(downloadStream, throttleStream, reply.raw);
      } else {
        await pipeline(downloadStream, reply.raw);
      }

      FilesystemStorageProvider.logMemoryUsage(`Download complete: ${objectName} (${downloadId})`);
    } catch (error) {
      console.error("Download error:", error);
      FilesystemStorageProvider.logMemoryUsage(`Download failed: ${objectName} (${downloadId})`);
      if (!reply.sent) {
        reply.status(500).send({ error: "Download failed" });
      }
    }
  }

  private async downloadFileRange(
    reply: FastifyReply,
    provider: FilesystemStorageProvider,
    objectName: string,
    start: number,
    end: number,
    downloadId?: string
  ) {
    try {
      FilesystemStorageProvider.logMemoryUsage(`Range download start: ${objectName} (${start}-${end}) (${downloadId})`);

      const rangeStream = await provider.createDownloadRangeStream(objectName, start, end);

      rangeStream.on("error", (error) => {
        console.error("Range download stream error:", error);
        FilesystemStorageProvider.logMemoryUsage(
          `Range download error: ${objectName} (${start}-${end}) (${downloadId})`
        );
        if (!reply.sent) {
          reply.status(500).send({ error: "Download failed" });
        }
      });

      reply.raw.on("close", () => {
        if (rangeStream.readable && typeof (rangeStream as any).destroy === "function") {
          (rangeStream as any).destroy();
        }
        FilesystemStorageProvider.logMemoryUsage(
          `Range download client disconnect: ${objectName} (${start}-${end}) (${downloadId})`
        );
      });

      await pipeline(rangeStream, reply.raw);

      FilesystemStorageProvider.logMemoryUsage(
        `Range download complete: ${objectName} (${start}-${end}) (${downloadId})`
      );
    } catch (error) {
      console.error("Range download error:", error);
      FilesystemStorageProvider.logMemoryUsage(
        `Range download failed: ${objectName} (${start}-${end}) (${downloadId})`
      );
      if (!reply.sent) {
        reply.status(500).send({ error: "Download failed" });
      }
    }
  }

  async getQueueStatus(_request: FastifyRequest, reply: FastifyReply) {
    try {
      const queueStatus = this.memoryManager.getQueueStatus();
      const response: QueueStatusResponse = {
        status: "success",
        data: queueStatus,
      };
      reply.status(200).send(response);
    } catch (error) {
      console.error("Error getting queue status:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  async cancelQueuedDownload(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { downloadId } = request.params as { downloadId: string };

      const cancelled = this.memoryManager.cancelQueuedDownload(downloadId);

      if (cancelled) {
        const response: DownloadCancelResponse = {
          message: "Download cancelled successfully",
          downloadId,
        };
        reply.status(200).send(response);
      } else {
        reply.status(404).send({
          error: "Download not found in queue",
          downloadId,
        });
      }
    } catch (error) {
      console.error("Error cancelling queued download:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  async clearDownloadQueue(_request: FastifyRequest, reply: FastifyReply) {
    try {
      const clearedCount = this.memoryManager.clearQueue();
      const response: QueueClearResponse = {
        message: "Download queue cleared successfully",
        clearedCount,
      };
      reply.status(200).send(response);
    } catch (error) {
      console.error("Error clearing download queue:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }
}
