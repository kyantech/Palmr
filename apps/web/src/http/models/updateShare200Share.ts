/**
 * Generated by orval v7.5.0 🍺
 * Do not edit manually.
 * 🌴 Palmr. API
 * API documentation for Palmr file sharing system
 * OpenAPI spec version: 1.0.0
 */
import type { UpdateShare200ShareAlias } from "./updateShare200ShareAlias";
import type { UpdateShare200ShareFilesItem } from "./updateShare200ShareFilesItem";
import type { UpdateShare200ShareRecipientsItem } from "./updateShare200ShareRecipientsItem";
import type { UpdateShare200ShareSecurity } from "./updateShare200ShareSecurity";

export type UpdateShare200Share = {
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
  security: UpdateShare200ShareSecurity;
  files: UpdateShare200ShareFilesItem[];
  recipients: UpdateShare200ShareRecipientsItem[];
  /** @nullable */
  alias: UpdateShare200ShareAlias;
};
