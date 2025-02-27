/**
 * Generated by orval v7.5.0 🍺
 * Do not edit manually.
 * 🌴 Palmr. API
 * API documentation for Palmr file sharing system
 * OpenAPI spec version: 1.0.0
 */

export type UpdateShareBody = {
  id: string;
  name?: string;
  description?: string;
  expiration?: string;
  password?: string;
  /** @nullable */
  maxViews?: number | null;
  recipients?: string[];
};
