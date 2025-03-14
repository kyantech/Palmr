/**
 * Generated by orval v7.5.0 🍺
 * Do not edit manually.
 * 🌴 Palmr. API
 * API documentation for Palmr file sharing system
 * OpenAPI spec version: 1.0.0
 */
import type { AddRecipients200ShareAlias } from "./addRecipients200ShareAlias";
import type { AddRecipients200ShareFilesItem } from "./addRecipients200ShareFilesItem";
import type { AddRecipients200ShareRecipientsItem } from "./addRecipients200ShareRecipientsItem";
import type { AddRecipients200ShareSecurity } from "./addRecipients200ShareSecurity";

export type AddRecipients200Share = {
  /** The share ID */
  id: string;
  /**
   * The share name
   * @nullable
   */
  name: string | null;
  /**
   * The share description
   * @nullable
   */
  description: string | null;
  /**
   * The share expiration date
   * @nullable
   */
  expiration: string | null;
  /** The number of views */
  views: number;
  /** The share creation date */
  createdAt: string;
  /** The share update date */
  updatedAt: string;
  /** The creator ID */
  creatorId: string;
  security: AddRecipients200ShareSecurity;
  files: AddRecipients200ShareFilesItem[];
  recipients: AddRecipients200ShareRecipientsItem[];
  /** @nullable */
  alias: AddRecipients200ShareAlias;
};
