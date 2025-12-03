import { getDownloadUrl } from "@/http/endpoints";
import { downloadReverseShareFile } from "@/http/endpoints/reverse-shares";

interface CacheEntry {
  url: string;
  expires: number;
  createdAt: number;
}

class DownloadUrlCache {
  private cache = new Map<string, CacheEntry>();

  // Presigned URLs expire in 3600s (1h)
  // We cache for 3300s (55min) with a 5min safety margin
  private readonly CACHE_DURATION = 3300 * 1000; // 55min in ms
  private readonly SAFETY_BUFFER = 60 * 1000; // 1min buffer before using cached URL

  /**
   * Generates unique cache key considering objectName and optional share password
   */
  private getCacheKey(objectName: string, options?: { headers?: { "x-share-password"?: string } }): string {
    const password = options?.headers?.["x-share-password"] || "";
    return password ? `${objectName}:${password}` : objectName;
  }

  /**
   * Checks if cache entry is still valid
   */
  private isValidCacheEntry(entry: CacheEntry): boolean {
    const now = Date.now();
    const timeUntilExpiration = entry.expires - now;
    return timeUntilExpiration > this.SAFETY_BUFFER;
  }

  /**
   * Removes expired entries from cache
   */
  private evictExpiredEntries(): void {
    const now = Date.now();

    for (const [key, entry] of this.cache.entries()) {
      if (entry.expires <= now) {
        this.cache.delete(key);
      }
    }
  }

  /**
   * Gets download URL with intelligent caching
   */
  async getCachedDownloadUrl(
    objectName: string,
    options?: { headers?: { "x-share-password"?: string } }
  ): Promise<string> {
    const cacheKey = this.getCacheKey(objectName, options);
    const now = Date.now();
    const cached = this.cache.get(cacheKey);

    if (cached && this.isValidCacheEntry(cached)) {
      return cached.url;
    }

    const response = await getDownloadUrl(objectName, options);
    const url = response.data.url;
    const entry: CacheEntry = {
      url,
      expires: now + this.CACHE_DURATION,
      createdAt: now,
    };

    this.cache.set(cacheKey, entry);

    if (this.cache.size % 10 === 0) {
      this.evictExpiredEntries();
    }

    return url;
  }

  /**
   * Gets download URL for reverse share with caching
   */
  async getCachedReverseShareDownloadUrl(fileId: string): Promise<string> {
    const cacheKey = `reverse:${fileId}`;
    const now = Date.now();
    const cached = this.cache.get(cacheKey);

    if (cached && this.isValidCacheEntry(cached)) {
      return cached.url;
    }

    const response = await downloadReverseShareFile(fileId);
    const url = response.data.url;
    const entry: CacheEntry = {
      url,
      expires: now + this.CACHE_DURATION,
      createdAt: now,
    };

    this.cache.set(cacheKey, entry);

    return url;
  }
}

// Singleton instance
export const downloadUrlCache = new DownloadUrlCache();

// Export main methods
export const getCachedDownloadUrl = downloadUrlCache.getCachedDownloadUrl.bind(downloadUrlCache);
export const getCachedReverseShareDownloadUrl =
  downloadUrlCache.getCachedReverseShareDownloadUrl.bind(downloadUrlCache);
