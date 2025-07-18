import * as crypto from "crypto";
import * as fs from "fs";
import { pipeline } from "stream/promises";
import { FastifyReply, FastifyRequest } from "fastify";

import { FilesystemStorageProvider } from "../../providers/filesystem-storage.provider";
import { ChunkManager, ChunkMetadata } from "./chunk-manager";
import { DownloadManager } from "./download-manager";
import { ResumableDownloadService } from "./resumable-download.service";
import { addStreamErrorHandlers } from "./stream-utils";

export class FilesystemController {
  private chunkManager = ChunkManager.getInstance();
  private downloadManager = DownloadManager.getInstance();
  private resumableDownloadService = ResumableDownloadService.getInstance();

  /**
   * Handle resumable session on download interruption
   * Pauses session for client disconnections (allows resumption)
   * Aborts session for server errors (prevents resumption)
   */
  private async handleResumableSessionInterruption(
    resumableSession: any,
    bytesTransferred: number,
    reason: string
  ): Promise<void> {
    if (!resumableSession) return;

    try {
      // Update progress first
      await this.resumableDownloadService.updateSessionProgress(resumableSession.sessionId, bytesTransferred);

      // Decide whether to pause (client issues) or abort (server errors)
      const isClientDisconnection =
        reason.includes("client") ||
        reason.includes("disconnection") ||
        reason.includes("close") ||
        reason.includes("aborted");

      if (isClientDisconnection) {
        console.log(
          `💾 Pausing resumable session ${resumableSession.sessionId} for resumption (${this.formatFileSize(bytesTransferred)} downloaded)`
        );
        await this.resumableDownloadService.pauseSession(resumableSession.sessionId, reason);
      } else {
        console.log(`❌ Aborting resumable session ${resumableSession.sessionId} due to server error (${reason})`);
        await this.resumableDownloadService.abortSession(resumableSession.sessionId, reason);
      }
    } catch (err) {
      console.error("Error handling resumable session interruption:", err);
    }
  }

  /**
   * Format file size for logging
   * @param bytes File size in bytes
   * @returns Formatted file size string
   */
  private formatFileSize(bytes: number): string {
    const units = ["B", "KB", "MB", "GB", "TB"];
    let size = bytes;
    let unitIndex = 0;

    while (size >= 1024 && unitIndex < units.length - 1) {
      size /= 1024;
      unitIndex++;
    }

    return `${size.toFixed(1)} ${units[unitIndex]}`;
  }

  /**
   * Safely encode filename for Content-Disposition header
   */
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

  /**
   * Extract chunk metadata from request headers
   */
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

  /**
   * Handle chunked upload with streaming
   */
  private async handleChunkedUpload(request: FastifyRequest, metadata: ChunkMetadata, originalObjectName: string) {
    const stream = request.raw;

    stream.on("error", (error) => {
      console.error("Request stream error:", error);
    });

    return await this.chunkManager.processChunk(metadata, stream, originalObjectName);
  }

  /**
   * Get upload progress for chunked uploads
   */
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

  /**
   * Cancel chunked upload
   */
  async cancelUpload(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { fileId } = request.params as { fileId: string };

      await this.chunkManager.cancelUpload(fileId);

      reply.status(200).send({ message: "Upload cancelled successfully" });
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  /**
   * Get resumable download session info
   */
  async getDownloadSession(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { sessionId } = request.params as { sessionId: string };

      const session = await this.resumableDownloadService.getSession(sessionId);

      if (!session) {
        return reply.status(404).send({ error: "Download session not found or expired" });
      }

      // Return session info without sensitive data
      const sessionInfo = {
        sessionId: session.sessionId,
        fileName: session.fileName,
        fileSize: session.fileSize,
        bytesDownloaded: session.bytesDownloaded,
        progress: (session.bytesDownloaded / session.fileSize) * 100,
        isActive: session.isActive,
        createdAt: session.createdAt,
        expiresAt: session.expiresAt,
        lastActivity: session.lastActivity,
      };

      reply.status(200).send(sessionInfo);
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  /**
   * Get all active download sessions for monitoring
   */
  async getActiveDownloadSessions(request: FastifyRequest, reply: FastifyReply) {
    try {
      const sessions = this.resumableDownloadService.getActiveSessions();
      const stats = this.resumableDownloadService.getSessionStats();

      const response = {
        stats,
        sessions: sessions.map((session) => ({
          sessionId: session.sessionId,
          fileName: session.fileName,
          fileSize: session.fileSize,
          bytesDownloaded: session.bytesDownloaded,
          progress: (session.bytesDownloaded / session.fileSize) * 100,
          isActive: session.isActive,
          createdAt: session.createdAt,
          lastActivity: session.lastActivity,
        })),
      };

      reply.status(200).send(response);
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  /**
   * Resume a download from where it left off
   */
  async resumeDownload(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { sessionId } = request.params as { sessionId: string };

      const session = await this.resumableDownloadService.getSession(sessionId);

      if (!session) {
        return reply.status(404).send({ error: "Download session not found or expired" });
      }

      // Validate that the client can resume this session
      const userAgent = request.headers["user-agent"] || "";
      const clientIPRaw = request.headers["x-real-ip"] || request.headers["x-forwarded-for"] || request.ip || "";
      const clientIP = Array.isArray(clientIPRaw) ? clientIPRaw[0] : clientIPRaw;
      const clientFingerprint = this.resumableDownloadService.generateClientFingerprint(userAgent, clientIP);

      if (session.clientFingerprint !== clientFingerprint) {
        return reply.status(403).send({ error: "Not authorized to resume this download session" });
      }

      // Create a range request to resume from where we left off
      const resumeFrom = session.bytesDownloaded;
      const fileSize = session.fileSize;

      console.log(
        `🔄 Resuming download session ${sessionId} from byte ${resumeFrom} (${this.formatFileSize(resumeFrom)} of ${this.formatFileSize(fileSize)})`
      );

      // Set up response headers for resumable download
      reply.header("Content-Disposition", this.encodeFilenameForHeader(session.fileName));
      reply.header("Content-Type", "application/octet-stream");
      reply.header("Accept-Ranges", "bytes");
      reply.header("Content-Range", `bytes ${resumeFrom}-${fileSize - 1}/${fileSize}`);
      reply.header("Content-Length", fileSize - resumeFrom);
      reply.header("X-Resumable-Session-Id", session.sessionId);
      reply.header("X-Bytes-Downloaded", session.bytesDownloaded.toString());
      reply.status(206); // Partial Content

      // Get the file provider and resume download
      const provider = FilesystemStorageProvider.getInstance();

      // Resume the download from the specified byte position
      await this.downloadLargeFileRangeResumable(
        reply,
        provider,
        session.objectName,
        resumeFrom,
        fileSize - 1,
        session
      );
    } catch (error) {
      console.error("Error resuming download:", error);
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  /**
   * Cancel a resumable download session
   */
  async cancelDownloadSession(request: FastifyRequest, reply: FastifyReply) {
    try {
      const { sessionId } = request.params as { sessionId: string };

      const session = await this.resumableDownloadService.getSession(sessionId);

      if (!session) {
        return reply.status(404).send({ error: "Download session not found" });
      }

      // Validate that the client can cancel this session
      const userAgent = request.headers["user-agent"] || "";
      const clientIPRaw = request.headers["x-real-ip"] || request.headers["x-forwarded-for"] || request.ip || "";
      const clientIP = Array.isArray(clientIPRaw) ? clientIPRaw[0] : clientIPRaw;
      const clientFingerprint = this.resumableDownloadService.generateClientFingerprint(userAgent, clientIP);

      if (session.clientFingerprint !== clientFingerprint) {
        return reply.status(403).send({ error: "Not authorized to cancel this download session" });
      }

      await this.resumableDownloadService.removeSession(sessionId);

      reply.status(200).send({ message: "Download session cancelled successfully" });
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
      const isLargeFile = fileSize > 50 * 1024 * 1024;

      const fileName = tokenData.fileName || "download";
      const range = request.headers.range;

      // Generate client fingerprint for resumable downloads
      const userAgent = request.headers["user-agent"] || "";
      const clientIPRaw = request.headers["x-real-ip"] || request.headers["x-forwarded-for"] || request.ip || "";
      const clientIP = Array.isArray(clientIPRaw) ? clientIPRaw[0] : clientIPRaw;
      const clientFingerprint = this.resumableDownloadService.generateClientFingerprint(userAgent, clientIP);

      // Check for existing resumable session
      let resumableSession = await this.resumableDownloadService.findSessionByToken(token, clientFingerprint);

      // Create resumable session for large files if it doesn't exist
      if (!resumableSession && isLargeFile) {
        resumableSession = await this.resumableDownloadService.createSession(
          token,
          tokenData.objectName,
          fileName,
          fileSize,
          clientFingerprint
        );
        console.log(
          `📁 Created resumable download session for large file: ${fileName} (${this.formatFileSize(fileSize)})`
        );
      }

      reply.header("Content-Disposition", this.encodeFilenameForHeader(fileName));
      reply.header("Content-Type", "application/octet-stream");
      reply.header("Accept-Ranges", "bytes");

      // Add resumable download headers
      if (resumableSession) {
        reply.header("X-Resumable-Session-Id", resumableSession.sessionId);
        reply.header("X-Bytes-Downloaded", resumableSession.bytesDownloaded.toString());
      }

      if (range) {
        const parts = range.replace(/bytes=/, "").split("-");
        const start = parseInt(parts[0], 10);
        const end = parts[1] ? parseInt(parts[1], 10) : fileSize - 1;
        const chunkSize = end - start + 1;

        reply.status(206);
        reply.header("Content-Range", `bytes ${start}-${end}/${fileSize}`);
        reply.header("Content-Length", chunkSize);

        // Log resumable download info
        if (resumableSession && start > 0) {
          console.log(
            `🔄 Resuming download from byte ${start}: ${resumableSession.sessionId} (${this.formatFileSize(start)} already downloaded)`
          );
        }

        if (isLargeFile) {
          await this.downloadLargeFileRangeResumable(
            reply,
            provider,
            tokenData.objectName,
            start,
            end,
            resumableSession
          );
        } else {
          const buffer = await provider.downloadFile(tokenData.objectName);
          const chunk = buffer.subarray(start, end + 1);
          reply.send(chunk);
        }
      } else {
        reply.header("Content-Length", fileSize);

        if (isLargeFile) {
          await this.downloadLargeFileResumable(reply, provider, filePath, resumableSession);
        } else {
          const buffer = await provider.downloadFile(tokenData.objectName);
          reply.send(buffer);
        }
      }

      provider.consumeDownloadToken(token);
    } catch (error) {
      return reply.status(500).send({ error: "Internal server error" });
    }
  }

  private async downloadLargeFile(reply: FastifyReply, provider: FilesystemStorageProvider, filePath: string) {
    // Generate unique download ID
    const downloadId = crypto.randomBytes(16).toString("hex");

    // Get file stats for tracking
    const stats = await fs.promises.stat(filePath);
    const fileSize = stats.size;

    // Start tracking the download
    this.downloadManager.startDownload(downloadId, filePath, fileSize);

    // Create streams
    const readStream = fs.createReadStream(filePath);
    const decryptStream = provider.createDecryptStream();

    // Register streams with download manager for cleanup
    this.downloadManager.registerStreams(downloadId, readStream, decryptStream);

    // Add error handlers to streams to prevent uncaught exceptions
    addStreamErrorHandlers(readStream, `download-${downloadId}-read`);
    addStreamErrorHandlers(decryptStream, `download-${downloadId}-decrypt`);

    // Track if download was aborted
    let isAborted = false;

    // Handle client disconnection events
    const handleAbort = (reason: string = "client_disconnected", error?: Error) => {
      if (!isAborted) {
        isAborted = true;
        console.log(`🔌 Client disconnected for download: ${downloadId} (${reason})`);
        this.downloadManager.abortDownload(downloadId, reason, error);
      }
    };

    // Listen for various client disconnection events
    reply.raw.on("close", () => handleAbort("client_close"));
    reply.raw.on("aborted", () => handleAbort("client_aborted"));
    reply.raw.on("error", (error) => {
      console.error(`❌ Response stream error for download ${downloadId}:`, error);
      handleAbort("response_stream_error", error);
    });

    try {
      // Execute the pipeline with proper error handling
      await pipeline(readStream, decryptStream, reply.raw);

      // If we reach here, download completed successfully
      if (!isAborted) {
        this.downloadManager.completeDownload(downloadId);
      }
    } catch (error: any) {
      // Handle pipeline errors
      if (!isAborted) {
        console.error(`❌ Pipeline error for download ${downloadId}:`, error);

        // Check if error is due to client disconnection
        if (error.code === "EPIPE" || error.code === "ECONNRESET" || error.message?.includes("aborted")) {
          console.log(`🔌 Download aborted due to client disconnection: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "pipeline_client_disconnection", error);
        } else {
          // Other errors - still abort the download but re-throw
          this.downloadManager.abortDownload(downloadId, "pipeline_error", error);
          throw error;
        }
      }
    } finally {
      // Clean up event listeners to prevent memory leaks
      try {
        reply.raw.removeListener("close", handleAbort);
        reply.raw.removeListener("aborted", handleAbort);
        reply.raw.removeAllListeners("error");
      } catch (cleanupError) {
        console.error(`⚠️ Error cleaning up event listeners for download ${downloadId}:`, cleanupError);
      }
    }
  }

  private async downloadLargeFileRange(
    reply: FastifyReply,
    provider: FilesystemStorageProvider,
    objectName: string,
    start: number,
    end: number
  ) {
    // Generate unique download ID for range request
    const downloadId = crypto.randomBytes(16).toString("hex");
    const chunkSize = end - start + 1;

    // Start tracking the range download
    this.downloadManager.startDownload(downloadId, objectName, chunkSize);

    console.log(
      `📥 Range download started: ${downloadId} (${objectName}, bytes ${start}-${end}, ${this.formatFileSize(chunkSize)})`
    );

    // Track if download was aborted
    let isAborted = false;

    // Handle client disconnection events
    const handleAbort = (reason: string = "client_disconnected", error?: Error) => {
      if (!isAborted) {
        isAborted = true;
        console.log(`🔌 Client disconnected for range download: ${downloadId} (${reason})`);
        this.downloadManager.abortDownload(downloadId, reason, error);
      }
    };

    // Listen for client disconnection events
    reply.raw.on("close", () => handleAbort("client_close"));
    reply.raw.on("aborted", () => handleAbort("client_aborted"));
    reply.raw.on("error", (error) => {
      console.error(`❌ Response stream error for range download ${downloadId}:`, error);
      handleAbort("response_stream_error", error);
    });

    try {
      // For very large range requests (>100MB), use streaming approach
      if (chunkSize > 100 * 1024 * 1024) {
        console.log(`📊 Using streaming approach for large range request: ${downloadId}`);
        await this.streamLargeFileRange(reply, provider, objectName, start, end, downloadId, isAborted);
      } else {
        // For smaller ranges, use buffer approach
        console.log(`📊 Using buffer approach for range request: ${downloadId}`);

        // Download the file buffer
        const buffer = await provider.downloadFile(objectName);

        // Check if client is still connected before processing
        if (isAborted || reply.raw.destroyed) {
          console.log(`🔌 Range download aborted before processing data: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "client_disconnected_before_processing");
          return;
        }

        // Extract the requested chunk using subarray for better memory efficiency
        const chunk = buffer.subarray(start, end + 1);

        // Check again before sending (client might have disconnected during processing)
        if (isAborted || reply.raw.destroyed) {
          console.log(`🔌 Range download aborted before sending data: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "client_disconnected_before_sending");
          return;
        }

        // Send the chunk
        reply.send(chunk);
      }

      // Mark as completed if not aborted
      if (!isAborted) {
        console.log(`✅ Range download completed: ${downloadId}`);
        this.downloadManager.completeDownload(downloadId);
      }
    } catch (error: any) {
      // Handle errors during range download
      if (!isAborted) {
        console.error(`❌ Error during range download ${downloadId}:`, error);

        // Check if error is due to client disconnection
        if (error.code === "EPIPE" || error.code === "ECONNRESET" || error.message?.includes("aborted")) {
          console.log(`🔌 Range download aborted due to client disconnection: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "range_client_disconnection", error);
        } else {
          // Other errors - still abort the download but re-throw
          this.downloadManager.abortDownload(downloadId, "range_error", error);
          throw error;
        }
      }
    } finally {
      // Clean up event listeners to prevent memory leaks
      try {
        reply.raw.removeListener("close", handleAbort);
        reply.raw.removeListener("aborted", handleAbort);
        reply.raw.removeAllListeners("error");
      } catch (cleanupError) {
        console.error(`⚠️ Error cleaning up event listeners for range download ${downloadId}:`, cleanupError);
      }
    }
  }

  /**
   * Stream a large file range to avoid loading large buffers into memory
   * @param reply Fastify reply object
   * @param provider Filesystem storage provider
   * @param objectName Object name to download
   * @param start Start byte position
   * @param end End byte position
   * @param downloadId Download ID for tracking
   * @param isAborted Reference to abort flag
   */
  private async streamLargeFileRange(
    reply: FastifyReply,
    provider: FilesystemStorageProvider,
    objectName: string,
    start: number,
    end: number,
    downloadId: string,
    isAborted: boolean
  ) {
    const filePath = provider.getFilePath(objectName);

    // Create a read stream for the specific range
    const readStream = fs.createReadStream(filePath, {
      start,
      end,
      highWaterMark: 64 * 1024 * 1024, // 64MB chunks for efficient streaming
    });

    // Create decrypt stream
    const decryptStream = provider.createDecryptStream();

    // Register streams with download manager for cleanup
    this.downloadManager.registerStreams(downloadId, readStream, decryptStream);

    // Add error handlers to streams to prevent uncaught exceptions
    addStreamErrorHandlers(readStream, `range-download-${downloadId}-read`);
    addStreamErrorHandlers(decryptStream, `range-download-${downloadId}-decrypt`);

    try {
      // Check if client is still connected before starting pipeline
      if (isAborted || reply.raw.destroyed) {
        console.log(`🔌 Range download aborted before streaming: ${downloadId}`);
        this.downloadManager.abortDownload(downloadId, "client_disconnected_before_streaming");
        return;
      }

      // Execute the pipeline with proper error handling
      await pipeline(readStream, decryptStream, reply.raw);

      console.log(`✅ Large range download streamed successfully: ${downloadId}`);
    } catch (error: any) {
      // Handle pipeline errors
      console.error(`❌ Pipeline error for range download ${downloadId}:`, error);

      // Check if error is due to client disconnection
      if (error.code === "EPIPE" || error.code === "ECONNRESET" || error.message?.includes("aborted")) {
        console.log(`🔌 Range download aborted due to client disconnection: ${downloadId}`);
      } else {
        // Re-throw other errors
        throw error;
      }
    }
  }

  /**
   * Download large file with resumable session support
   * @param reply Fastify reply object
   * @param provider Filesystem storage provider
   * @param filePath Path to the file
   * @param resumableSession Optional resumable session
   */
  private async downloadLargeFileResumable(
    reply: FastifyReply,
    provider: FilesystemStorageProvider,
    filePath: string,
    resumableSession?: any
  ) {
    // Generate unique download ID
    const downloadId = crypto.randomBytes(16).toString("hex");

    // Get file stats for tracking
    const stats = await fs.promises.stat(filePath);
    const fileSize = stats.size;

    // Start tracking the download
    this.downloadManager.startDownload(downloadId, filePath, fileSize);

    // Create streams
    const readStream = fs.createReadStream(filePath);
    const decryptStream = provider.createDecryptStream();

    // Register streams with download manager for cleanup
    this.downloadManager.registerStreams(downloadId, readStream, decryptStream);

    // Add error handlers to streams to prevent uncaught exceptions
    addStreamErrorHandlers(readStream, `download-${downloadId}-read`);
    addStreamErrorHandlers(decryptStream, `download-${downloadId}-decrypt`);

    // Track if download was aborted
    let isAborted = false;
    let bytesTransferred = 0;

    // Handle client disconnection events
    const handleAbort = async (reason: string = "client_disconnected", error?: Error) => {
      if (!isAborted) {
        isAborted = true;
        console.log(`🔌 Client disconnected for download: ${downloadId} (${reason})`);

        // Handle resumable session interruption
        await this.handleResumableSessionInterruption(resumableSession, bytesTransferred, reason);

        this.downloadManager.abortDownload(downloadId, reason, error);
      }
    };

    // Listen for various client disconnection events
    reply.raw.on("close", () => handleAbort("client_close"));
    reply.raw.on("aborted", () => handleAbort("client_aborted"));
    reply.raw.on("error", (error) => {
      console.error(`❌ Response stream error for download ${downloadId}:`, error);
      handleAbort("response_stream_error", error);
    });

    // Track progress if resumable session exists
    if (resumableSession) {
      const progressTracker = setInterval(() => {
        if (!isAborted && resumableSession) {
          this.resumableDownloadService
            .updateSessionProgress(resumableSession.sessionId, bytesTransferred)
            .catch((err) => console.error("Error updating session progress:", err));
        }
      }, 5000); // Update every 5 seconds

      // Clean up interval on completion/abort
      const originalHandleAbort = handleAbort;
      const handleAbortWithCleanup = (reason?: string, error?: Error) => {
        clearInterval(progressTracker);
        originalHandleAbort(reason, error);
      };

      reply.raw.removeAllListeners("close");
      reply.raw.removeAllListeners("aborted");
      reply.raw.on("close", () => handleAbortWithCleanup("client_close"));
      reply.raw.on("aborted", () => handleAbortWithCleanup("client_aborted"));
    }

    try {
      // Track bytes transferred
      readStream.on("data", (chunk) => {
        bytesTransferred += chunk.length;
        this.downloadManager.updateBytesTransferred(downloadId, bytesTransferred);

        // Update resumable session progress in real-time (throttled)
        if (resumableSession) {
          // Throttle updates to avoid too many writes (every 10MB or on first chunk)
          if (bytesTransferred % (10 * 1024 * 1024) < chunk.length || bytesTransferred === chunk.length) {
            this.resumableDownloadService
              .updateSessionProgress(resumableSession.sessionId, bytesTransferred)
              .catch((err) => console.error("Error updating session progress:", err));
          }
        }
      });

      // Execute the pipeline with proper error handling
      await pipeline(readStream, decryptStream, reply.raw);

      // If we reach here, download completed successfully
      if (!isAborted) {
        this.downloadManager.completeDownload(downloadId);

        // Complete resumable session if available
        if (resumableSession) {
          await this.resumableDownloadService.completeSession(resumableSession.sessionId);
        }
      }
    } catch (error: any) {
      // Handle pipeline errors
      if (!isAborted) {
        console.error(`❌ Pipeline error for download ${downloadId}:`, error);

        // Check if error is due to client disconnection
        if (error.code === "EPIPE" || error.code === "ECONNRESET" || error.message?.includes("aborted")) {
          console.log(`🔌 Download aborted due to client disconnection: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "pipeline_client_disconnection", error);

          // Update resumable session
          if (resumableSession) {
            await this.resumableDownloadService.updateSessionProgress(resumableSession.sessionId, bytesTransferred);
            await this.resumableDownloadService.abortSession(
              resumableSession.sessionId,
              "pipeline_client_disconnection"
            );
          }
        } else {
          // Other errors - still abort the download but re-throw
          this.downloadManager.abortDownload(downloadId, "pipeline_error", error);

          if (resumableSession) {
            await this.resumableDownloadService.abortSession(resumableSession.sessionId, "pipeline_error");
          }

          throw error;
        }
      }
    } finally {
      // Clean up event listeners to prevent memory leaks
      try {
        reply.raw.removeListener("close", handleAbort);
        reply.raw.removeListener("aborted", handleAbort);
        reply.raw.removeAllListeners("error");
      } catch (cleanupError) {
        console.error(`⚠️ Error cleaning up event listeners for download ${downloadId}:`, cleanupError);
      }
    }
  }

  /**
   * Download large file range with resumable session support
   * @param reply Fastify reply object
   * @param provider Filesystem storage provider
   * @param objectName Object name to download
   * @param start Start byte position
   * @param end End byte position
   * @param resumableSession Optional resumable session
   */
  private async downloadLargeFileRangeResumable(
    reply: FastifyReply,
    provider: FilesystemStorageProvider,
    objectName: string,
    start: number,
    end: number,
    resumableSession?: any
  ) {
    // Generate unique download ID for range request
    const downloadId = crypto.randomBytes(16).toString("hex");
    const chunkSize = end - start + 1;

    // Start tracking the range download
    this.downloadManager.startDownload(downloadId, objectName, chunkSize);

    console.log(
      `📥 Resumable range download started: ${downloadId} (${objectName}, bytes ${start}-${end}, ${this.formatFileSize(chunkSize)})`
    );

    // Track if download was aborted
    let isAborted = false;
    let bytesTransferred = start; // Start from the range start

    // Handle client disconnection events
    const handleAbort = (reason: string = "client_disconnected", error?: Error) => {
      if (!isAborted) {
        isAborted = true;
        console.log(`🔌 Client disconnected for range download: ${downloadId} (${reason})`);

        // Update resumable session if available
        if (resumableSession) {
          this.resumableDownloadService
            .updateSessionProgress(resumableSession.sessionId, bytesTransferred)
            .then(() => {
              this.resumableDownloadService.abortSession(resumableSession.sessionId, reason);
            })
            .catch((err) => console.error("Error updating session on abort:", err));
        }

        this.downloadManager.abortDownload(downloadId, reason, error);
      }
    };

    // Listen for client disconnection events
    reply.raw.on("close", () => handleAbort("client_close"));
    reply.raw.on("aborted", () => handleAbort("client_aborted"));
    reply.raw.on("error", (error) => {
      console.error(`❌ Response stream error for range download ${downloadId}:`, error);
      handleAbort("response_stream_error", error);
    });

    try {
      // For very large range requests (>100MB), use streaming approach
      if (chunkSize > 100 * 1024 * 1024) {
        console.log(`📊 Using streaming approach for large resumable range request: ${downloadId}`);
        await this.streamLargeFileRangeResumable(reply, provider, objectName, start, end, downloadId, resumableSession);
      } else {
        // For smaller ranges, use buffer approach
        console.log(`📊 Using buffer approach for resumable range request: ${downloadId}`);

        // Download the file buffer
        const buffer = await provider.downloadFile(objectName);

        // Check if client is still connected before processing
        if (isAborted || reply.raw.destroyed) {
          console.log(`🔌 Range download aborted before processing data: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "client_disconnected_before_processing");
          return;
        }

        // Extract the requested chunk using subarray for better memory efficiency
        const chunk = buffer.subarray(start, end + 1);

        // Check again before sending (client might have disconnected during processing)
        if (isAborted || reply.raw.destroyed) {
          console.log(`🔌 Range download aborted before sending data: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "client_disconnected_before_sending");
          return;
        }

        // Send the chunk
        reply.send(chunk);
        bytesTransferred = end + 1;
      }

      // Mark as completed if not aborted
      if (!isAborted) {
        console.log(`✅ Resumable range download completed: ${downloadId}`);
        this.downloadManager.completeDownload(downloadId);

        // Update resumable session progress
        if (resumableSession) {
          await this.resumableDownloadService.updateSessionProgress(resumableSession.sessionId, bytesTransferred);
        }
      }
    } catch (error: any) {
      // Handle errors during range download
      if (!isAborted) {
        console.error(`❌ Error during resumable range download ${downloadId}:`, error);

        // Check if error is due to client disconnection
        if (error.code === "EPIPE" || error.code === "ECONNRESET" || error.message?.includes("aborted")) {
          console.log(`🔌 Range download aborted due to client disconnection: ${downloadId}`);
          this.downloadManager.abortDownload(downloadId, "range_client_disconnection", error);

          if (resumableSession) {
            await this.resumableDownloadService.updateSessionProgress(resumableSession.sessionId, bytesTransferred);
            await this.resumableDownloadService.abortSession(resumableSession.sessionId, "range_client_disconnection");
          }
        } else {
          // Other errors - still abort the download but re-throw
          this.downloadManager.abortDownload(downloadId, "range_error", error);

          if (resumableSession) {
            await this.resumableDownloadService.abortSession(resumableSession.sessionId, "range_error");
          }

          throw error;
        }
      }
    } finally {
      // Clean up event listeners to prevent memory leaks
      try {
        reply.raw.removeListener("close", handleAbort);
        reply.raw.removeListener("aborted", handleAbort);
        reply.raw.removeAllListeners("error");
      } catch (cleanupError) {
        console.error(`⚠️ Error cleaning up event listeners for range download ${downloadId}:`, cleanupError);
      }
    }
  }

  /**
   * Stream a large file range with resumable session support
   * @param reply Fastify reply object
   * @param provider Filesystem storage provider
   * @param objectName Object name to download
   * @param start Start byte position
   * @param end End byte position
   * @param downloadId Download ID for tracking
   * @param resumableSession Optional resumable session
   */
  private async streamLargeFileRangeResumable(
    reply: FastifyReply,
    provider: FilesystemStorageProvider,
    objectName: string,
    start: number,
    end: number,
    downloadId: string,
    resumableSession?: any
  ) {
    const filePath = provider.getFilePath(objectName);
    let bytesTransferred = start;

    // Create a read stream for the specific range
    const readStream = fs.createReadStream(filePath, {
      start,
      end,
      highWaterMark: 64 * 1024 * 1024, // 64MB chunks for efficient streaming
    });

    // Create decrypt stream
    const decryptStream = provider.createDecryptStream();

    // Register streams with download manager for cleanup
    this.downloadManager.registerStreams(downloadId, readStream, decryptStream);

    // Add error handlers to streams to prevent uncaught exceptions
    addStreamErrorHandlers(readStream, `range-download-${downloadId}-read`);
    addStreamErrorHandlers(decryptStream, `range-download-${downloadId}-decrypt`);

    // Track progress if resumable session exists
    if (resumableSession) {
      readStream.on("data", (chunk) => {
        bytesTransferred += chunk.length;
        this.downloadManager.updateBytesTransferred(downloadId, bytesTransferred);
      });

      const progressTracker = setInterval(() => {
        if (resumableSession) {
          this.resumableDownloadService
            .updateSessionProgress(resumableSession.sessionId, bytesTransferred)
            .catch((err) => console.error("Error updating session progress:", err));
        }
      }, 5000); // Update every 5 seconds

      // Clean up interval when stream ends
      readStream.on("end", () => clearInterval(progressTracker));
      readStream.on("error", () => clearInterval(progressTracker));
    }

    try {
      // Check if client is still connected before starting pipeline
      if (reply.raw.destroyed) {
        console.log(`🔌 Range download aborted before streaming: ${downloadId}`);
        this.downloadManager.abortDownload(downloadId, "client_disconnected_before_streaming");
        return;
      }

      // Execute the pipeline with proper error handling
      await pipeline(readStream, decryptStream, reply.raw);

      console.log(`✅ Large resumable range download streamed successfully: ${downloadId}`);

      // Update final progress
      if (resumableSession) {
        await this.resumableDownloadService.updateSessionProgress(resumableSession.sessionId, end + 1);
      }
    } catch (error: any) {
      // Handle pipeline errors
      console.error(`❌ Pipeline error for resumable range download ${downloadId}:`, error);

      // Update session with current progress
      if (resumableSession) {
        await this.resumableDownloadService.updateSessionProgress(resumableSession.sessionId, bytesTransferred);
      }

      // Check if error is due to client disconnection
      if (error.code === "EPIPE" || error.code === "ECONNRESET" || error.message?.includes("aborted")) {
        console.log(`🔌 Range download aborted due to client disconnection: ${downloadId}`);

        if (resumableSession) {
          await this.resumableDownloadService.abortSession(resumableSession.sessionId, "pipeline_client_disconnection");
        }
      } else {
        // Re-throw other errors
        if (resumableSession) {
          await this.resumableDownloadService.abortSession(resumableSession.sessionId, "pipeline_error");
        }
        throw error;
      }
    }
  }
}
