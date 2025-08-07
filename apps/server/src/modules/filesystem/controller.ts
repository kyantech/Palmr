import * as fs from "fs";
import { pipeline } from "stream/promises";
import { FastifyReply, FastifyRequest } from "fastify";

import { FilesystemStorageProvider } from "../../providers/filesystem-storage.provider";
import { ChunkManager, ChunkMetadata } from "./chunk-manager";

export class FilesystemController {
  private chunkManager = ChunkManager.getInstance();

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

    const asciiSafe = sanitized
      .split("")
      .filter((char) => {
        const code = char.charCodeAt(0);
        return code >= 32 && code <= 126;
      })
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
    const fileName = request.headers["x-file-name"] as string;
    const isLastChunk = request.headers["x-is-last-chunk"] as string;

    if (!fileId || !chunkIndex || !totalChunks || !chunkSize || !totalSize || !fileName) {
      return null;
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
    try {
      const { token } = request.params as { token: string };

      const provider = FilesystemStorageProvider.getInstance();

      const tokenData = provider.validateDownloadToken(token);

      if (!tokenData) {
        return reply.status(400).send({ error: "Invalid or expired download token" });
      }

      const filePath = provider.getFilePath(tokenData.objectName);
      const stats = await fs.promises.stat(filePath);
      const fileSize = stats.size;

      const fileName = tokenData.fileName || "download";
      const range = request.headers.range;

      reply.header("Content-Disposition", this.encodeFilenameForHeader(fileName));
      reply.header("Content-Type", "application/octet-stream");
      reply.header("Accept-Ranges", "bytes");

      if (range) {
        const parts = range.replace(/bytes=/, "").split("-");
        const start = parseInt(parts[0], 10);
        const end = parts[1] ? parseInt(parts[1], 10) : fileSize - 1;

        reply.status(206);
        reply.header("Content-Range", `bytes ${start}-${end}/${fileSize}`);
        reply.header("Content-Length", end - start + 1);

        await this.downloadFileRange(reply, provider, tokenData.objectName, start, end);
      } else {
        reply.header("Content-Length", fileSize);
        await this.downloadFileStream(reply, provider, tokenData.objectName);
      }

      provider.consumeDownloadToken(token);
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  private async downloadFileStream(reply: FastifyReply, provider: FilesystemStorageProvider, objectName: string) {
    try {
      FilesystemStorageProvider.logMemoryUsage(`Download start: ${objectName}`);

      const downloadStream = provider.createDownloadStream(objectName);

      downloadStream.on("error", (error) => {
        console.error("Download stream error:", error);
        FilesystemStorageProvider.logMemoryUsage(`Download error: ${objectName}`);
        if (!reply.sent) {
          reply.status(500).send({ error: "Download failed" });
        }
      });

      reply.raw.on("close", () => {
        if (downloadStream.readable && typeof (downloadStream as any).destroy === "function") {
          (downloadStream as any).destroy();
        }
        FilesystemStorageProvider.logMemoryUsage(`Download client disconnect: ${objectName}`);
      });

      await pipeline(downloadStream, reply.raw);

      FilesystemStorageProvider.logMemoryUsage(`Download complete: ${objectName}`);
    } catch (error) {
      console.error("Download error:", error);
      FilesystemStorageProvider.logMemoryUsage(`Download failed: ${objectName}`);
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
    end: number
  ) {
    try {
      FilesystemStorageProvider.logMemoryUsage(`Range download start: ${objectName} (${start}-${end})`);

      const rangeStream = await provider.createDownloadRangeStream(objectName, start, end);

      rangeStream.on("error", (error) => {
        console.error("Range download stream error:", error);
        FilesystemStorageProvider.logMemoryUsage(`Range download error: ${objectName} (${start}-${end})`);
        if (!reply.sent) {
          reply.status(500).send({ error: "Download failed" });
        }
      });

      reply.raw.on("close", () => {
        if (rangeStream.readable && typeof (rangeStream as any).destroy === "function") {
          (rangeStream as any).destroy();
        }
        FilesystemStorageProvider.logMemoryUsage(`Range download client disconnect: ${objectName} (${start}-${end})`);
      });

      await pipeline(rangeStream, reply.raw);

      FilesystemStorageProvider.logMemoryUsage(`Range download complete: ${objectName} (${start}-${end})`);
    } catch (error) {
      console.error("Range download error:", error);
      FilesystemStorageProvider.logMemoryUsage(`Range download failed: ${objectName} (${start}-${end})`);
      if (!reply.sent) {
        reply.status(500).send({ error: "Download failed" });
      }
    }
  }
}
