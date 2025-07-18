import * as fs from "fs/promises";
import * as path from "path";

import { directoriesConfig } from "../../config/directories.config";

/**
 * Information about a resumable download session
 */
export interface ResumableDownloadSession {
  sessionId: string;
  downloadToken: string;
  objectName: string;
  fileName: string;
  fileSize: number;
  bytesDownloaded: number;
  lastActivity: number;
  clientFingerprint: string;
  createdAt: number;
  expiresAt: number;
  isActive: boolean;
}

/**
 * Service for managing resumable download sessions
 * Handles persistence and recovery of interrupted downloads
 */
export class ResumableDownloadService {
  private static instance: ResumableDownloadService;
  private sessionsDir: string;
  private activeSessions = new Map<string, ResumableDownloadSession>();
  private readonly SESSION_EXPIRY_HOURS = 24; // 24 hours
  private readonly CLEANUP_INTERVAL_MS = 60 * 60 * 1000; // 1 hour
  private cleanupInterval?: NodeJS.Timeout;

  private constructor() {
    this.sessionsDir = path.join(directoriesConfig.tempUploads, "download-sessions");
    this.initializeService();
  }

  /**
   * Get the singleton instance
   */
  public static getInstance(): ResumableDownloadService {
    if (!ResumableDownloadService.instance) {
      ResumableDownloadService.instance = new ResumableDownloadService();
    }
    return ResumableDownloadService.instance;
  }

  /**
   * Initialize the service
   */
  private async initializeService(): Promise<void> {
    try {
      // Ensure sessions directory exists
      await fs.mkdir(this.sessionsDir, { recursive: true });

      // Load existing sessions
      await this.loadExistingSessions();

      // Start cleanup interval
      this.startCleanupInterval();

      console.log(`📁 Resumable download service initialized (${this.activeSessions.size} active sessions)`);
    } catch (error) {
      console.error("❌ Error initializing resumable download service:", error);
    }
  }

  /**
   * Create a new resumable download session
   */
  async createSession(
    downloadToken: string,
    objectName: string,
    fileName: string,
    fileSize: number,
    clientFingerprint: string
  ): Promise<ResumableDownloadSession> {
    const sessionId = this.generateSessionId();
    const now = Date.now();
    const expiresAt = now + this.SESSION_EXPIRY_HOURS * 60 * 60 * 1000;

    const session: ResumableDownloadSession = {
      sessionId,
      downloadToken,
      objectName,
      fileName,
      fileSize,
      bytesDownloaded: 0,
      lastActivity: now,
      clientFingerprint,
      createdAt: now,
      expiresAt,
      isActive: true,
    };

    // Store in memory
    this.activeSessions.set(sessionId, session);

    // Persist to disk
    await this.persistSession(session);

    console.log(`📥 Created resumable download session: ${sessionId} (${fileName}, ${this.formatFileSize(fileSize)})`);

    return session;
  }

  /**
   * Get an existing session by ID
   */
  async getSession(sessionId: string): Promise<ResumableDownloadSession | null> {
    // Check memory first
    let session = this.activeSessions.get(sessionId);

    if (!session) {
      // Try to load from disk
      const diskSession = await this.loadSessionFromDisk(sessionId);
      if (diskSession) {
        session = diskSession;
        this.activeSessions.set(sessionId, session);
      }
    }

    // Check if session is expired
    if (session && session.expiresAt < Date.now()) {
      console.log(`⏰ Session expired: ${sessionId}`);
      await this.removeSession(sessionId);
      return null;
    }

    return session || null;
  }

  /**
   * Find session by download token and client fingerprint
   */
  async findSessionByToken(downloadToken: string, clientFingerprint: string): Promise<ResumableDownloadSession | null> {
    // Check active sessions first
    for (const session of this.activeSessions.values()) {
      if (session.downloadToken === downloadToken && session.clientFingerprint === clientFingerprint) {
        return session;
      }
    }

    // Load all sessions from disk and check
    const sessionFiles = await this.getSessionFiles();
    for (const file of sessionFiles) {
      const session = await this.loadSessionFromDisk(path.basename(file, ".json"));
      if (
        session &&
        session.downloadToken === downloadToken &&
        session.clientFingerprint === clientFingerprint &&
        session.expiresAt > Date.now()
      ) {
        this.activeSessions.set(session.sessionId, session);
        return session;
      }
    }

    return null;
  }

  /**
   * Update session progress
   */
  async updateSessionProgress(sessionId: string, bytesDownloaded: number): Promise<void> {
    const session = await this.getSession(sessionId);
    if (!session) {
      console.warn(`⚠️ Attempted to update unknown session: ${sessionId}`);
      return;
    }

    session.bytesDownloaded = bytesDownloaded;
    session.lastActivity = Date.now();

    // Update in memory
    this.activeSessions.set(sessionId, session);

    // Persist to disk (throttled to avoid too many writes)
    await this.persistSession(session);

    // Log progress periodically
    const progressPercent = (bytesDownloaded / session.fileSize) * 100;
    if (progressPercent % 10 < 1) {
      // Log every ~10%
      console.log(
        `📊 Download progress ${sessionId}: ${progressPercent.toFixed(1)}% (${this.formatFileSize(bytesDownloaded)}/${this.formatFileSize(session.fileSize)})`
      );
    }
  }

  /**
   * Complete a download session
   */
  async completeSession(sessionId: string): Promise<void> {
    const session = await this.getSession(sessionId);
    if (!session) {
      console.warn(`⚠️ Attempted to complete unknown session: ${sessionId}`);
      return;
    }

    console.log(`✅ Download session completed: ${sessionId} (${session.fileName})`);
    await this.removeSession(sessionId);
  }

  /**
   * Pause a download session (for client disconnections - allows resumption)
   */
  async pauseSession(sessionId: string, reason?: string): Promise<void> {
    const session = await this.getSession(sessionId);
    if (!session) {
      console.warn(`⚠️ Attempted to pause unknown session: ${sessionId}`);
      return;
    }

    console.log(`⏸️ Download session paused: ${sessionId} (${reason || "unknown reason"}) - can be resumed`);

    // Mark as inactive but keep for resumption
    session.isActive = false;
    session.lastActivity = Date.now();

    this.activeSessions.set(sessionId, session);
    await this.persistSession(session);
  }

  /**
   * Abort a download session (for server errors - prevents resumption)
   */
  async abortSession(sessionId: string, reason?: string): Promise<void> {
    const session = await this.getSession(sessionId);
    if (!session) {
      console.warn(`⚠️ Attempted to abort unknown session: ${sessionId}`);
      return;
    }

    console.log(`❌ Download session aborted: ${sessionId} (${reason || "unknown reason"}) - cannot be resumed`);

    // Remove session completely for server errors
    await this.removeSession(sessionId);
  }

  /**
   * Remove a session completely
   */
  async removeSession(sessionId: string): Promise<void> {
    // Remove from memory
    this.activeSessions.delete(sessionId);

    // Remove from disk
    try {
      const sessionFile = path.join(this.sessionsDir, `${sessionId}.json`);
      await fs.unlink(sessionFile);
    } catch (error) {
      // File might not exist, that's okay
    }
  }

  /**
   * Get all active sessions
   */
  getActiveSessions(): ResumableDownloadSession[] {
    return Array.from(this.activeSessions.values()).filter((session) => session.isActive);
  }

  /**
   * Get session statistics
   */
  getSessionStats(): {
    totalSessions: number;
    activeSessions: number;
    totalBytes: number;
    downloadedBytes: number;
  } {
    const sessions = Array.from(this.activeSessions.values());
    const activeSessions = sessions.filter((s) => s.isActive);

    return {
      totalSessions: sessions.length,
      activeSessions: activeSessions.length,
      totalBytes: sessions.reduce((sum, s) => sum + s.fileSize, 0),
      downloadedBytes: sessions.reduce((sum, s) => sum + s.bytesDownloaded, 0),
    };
  }

  /**
   * Generate client fingerprint from request
   */
  generateClientFingerprint(userAgent?: string, ipAddress?: string): string {
    const crypto = require("crypto");
    const data = `${userAgent || "unknown"}-${ipAddress || "unknown"}-${Date.now()}`;
    return crypto.createHash("sha256").update(data).digest("hex").substring(0, 16);
  }

  /**
   * Cleanup expired sessions
   */
  async cleanupExpiredSessions(): Promise<void> {
    const now = Date.now();
    let cleanedCount = 0;

    console.log(`🧹 Starting expired session cleanup (${this.activeSessions.size} sessions)`);

    // Cleanup memory sessions
    for (const [sessionId, session] of this.activeSessions.entries()) {
      if (session.expiresAt < now) {
        await this.removeSession(sessionId);
        cleanedCount++;
      }
    }

    // Cleanup disk sessions
    try {
      const sessionFiles = await this.getSessionFiles();
      for (const file of sessionFiles) {
        const sessionId = path.basename(file, ".json");
        const session = await this.loadSessionFromDisk(sessionId);
        if (session && session.expiresAt < now) {
          await this.removeSession(sessionId);
          cleanedCount++;
        }
      }
    } catch (error) {
      console.error("❌ Error during session cleanup:", error);
    }

    if (cleanedCount > 0) {
      console.log(`🧹 Cleaned up ${cleanedCount} expired download sessions`);
    } else {
      console.log(`🧹 No expired download sessions found`);
    }
  }

  /**
   * Start cleanup interval
   */
  private startCleanupInterval(): void {
    this.cleanupInterval = setInterval(() => {
      this.cleanupExpiredSessions().catch((error) => {
        console.error("❌ Error in session cleanup interval:", error);
      });
    }, this.CLEANUP_INTERVAL_MS);
  }

  /**
   * Stop cleanup interval
   */
  stopCleanupInterval(): void {
    if (this.cleanupInterval) {
      clearInterval(this.cleanupInterval);
      this.cleanupInterval = undefined;
    }
  }

  /**
   * Load existing sessions from disk
   */
  private async loadExistingSessions(): Promise<void> {
    try {
      const sessionFiles = await this.getSessionFiles();
      let loadedCount = 0;

      for (const file of sessionFiles) {
        const sessionId = path.basename(file, ".json");
        const session = await this.loadSessionFromDisk(sessionId);
        if (session && session.expiresAt > Date.now()) {
          this.activeSessions.set(sessionId, session);
          loadedCount++;
        }
      }

      if (loadedCount > 0) {
        console.log(`📁 Loaded ${loadedCount} existing download sessions`);
      }
    } catch (error) {
      console.error("❌ Error loading existing sessions:", error);
    }
  }

  /**
   * Get all session files
   */
  private async getSessionFiles(): Promise<string[]> {
    try {
      const files = await fs.readdir(this.sessionsDir);
      return files.filter((file) => file.endsWith(".json")).map((file) => path.join(this.sessionsDir, file));
    } catch (error) {
      return [];
    }
  }

  /**
   * Load session from disk
   */
  private async loadSessionFromDisk(sessionId: string): Promise<ResumableDownloadSession | null> {
    try {
      const sessionFile = path.join(this.sessionsDir, `${sessionId}.json`);
      const data = await fs.readFile(sessionFile, "utf-8");
      return JSON.parse(data) as ResumableDownloadSession;
    } catch (error) {
      return null;
    }
  }

  /**
   * Persist session to disk
   */
  private async persistSession(session: ResumableDownloadSession): Promise<void> {
    try {
      const sessionFile = path.join(this.sessionsDir, `${session.sessionId}.json`);
      await fs.writeFile(sessionFile, JSON.stringify(session, null, 2));
    } catch (error) {
      console.error(`❌ Error persisting session ${session.sessionId}:`, error);
    }
  }

  /**
   * Generate unique session ID
   */
  private generateSessionId(): string {
    const crypto = require("crypto");
    return crypto.randomBytes(16).toString("hex");
  }

  /**
   * Format file size for logging
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
   * Shutdown the service
   */
  async shutdown(): Promise<void> {
    console.log("🛑 Shutting down resumable download service");
    this.stopCleanupInterval();

    // Persist all active sessions
    for (const session of this.activeSessions.values()) {
      await this.persistSession(session);
    }

    console.log("✅ Resumable download service shutdown complete");
  }
}
