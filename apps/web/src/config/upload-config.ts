/**
 * Upload configuration for the application
 * Centralizes all upload-related settings
 */
export const UPLOAD_CONFIG = {
  /**
   * Size threshold for multipart upload (100MB)
   * Files >= this size will use multipart upload
   * Files < this size will use simple PUT upload
   */
  MULTIPART_THRESHOLD: 50 * 1024 * 1024,

  /**
   * No file size limit (managed by backend/user quota)
   */
  MAX_FILE_SIZE: null,

  /**
   * No file count limit (configurable per context)
   */
  MAX_FILES: null,

  /**
   * Allow all file types (restrictions are context-specific)
   */
  ALLOWED_TYPES: null,

  /**
   * Retry configuration
   */
  MAX_RETRIES: 5,
  RETRY_DELAYS: [1000, 3000, 5000, 10000, 15000], // ms

  /**
   * Concurrent uploads (unlimited/maximum possible)
   * 0 = unlimited
   */
  MAX_CONCURRENT: 0,

  /**
   * Progress update debounce (100ms for performance)
   */
  PROGRESS_DEBOUNCE: 100,

  /**
   * "Many files" threshold for UI optimizations
   */
  MANY_FILES_THRESHOLD: 100,

  /**
   * Toast auto-dismiss duration
   */
  TOAST_DURATION: 3000, // ms

  /**
   * Preview generation
   */
  PREVIEW_MAX_SIZE: null, // No limit
  PREVIEW_TYPES: ["image/*"],
} as const;

export type UploadConfig = typeof UPLOAD_CONFIG;
