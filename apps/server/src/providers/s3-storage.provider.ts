import { DeleteObjectCommand, GetObjectCommand, PutObjectCommand } from "@aws-sdk/client-s3";
import { getSignedUrl } from "@aws-sdk/s3-request-presigner";

import { bucketName, s3Client } from "../config/storage.config";
import { StorageProvider } from "../types/storage";

export class S3StorageProvider implements StorageProvider {
  constructor() {
    if (!s3Client) {
      throw new Error(
        "S3 client is not configured. Make sure ENABLE_S3=true and all S3 environment variables are set."
      );
    }
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

  async getPresignedPutUrl(objectName: string, expires: number): Promise<string> {
    if (!s3Client) {
      throw new Error("S3 client is not available");
    }

    const command = new PutObjectCommand({
      Bucket: bucketName,
      Key: objectName,
    });

    return await getSignedUrl(s3Client, command, { expiresIn: expires });
  }

  async getPresignedGetUrl(objectName: string, expires: number, fileName?: string): Promise<string> {
    if (!s3Client) {
      throw new Error("S3 client is not available");
    }

    let rcdFileName: string;

    if (fileName && fileName.trim() !== "") {
      rcdFileName = fileName;
    } else {
      const lastSlashIndex = objectName.lastIndexOf("/");
      rcdFileName = lastSlashIndex !== -1 ? objectName.substring(lastSlashIndex + 1) : objectName;
      if (!rcdFileName) {
        rcdFileName = "downloaded_file";
      }
    }

    const command = new GetObjectCommand({
      Bucket: bucketName,
      Key: objectName,
      ResponseContentDisposition: this.encodeFilenameForHeader(rcdFileName),
    });

    return await getSignedUrl(s3Client, command, { expiresIn: expires });
  }

  async deleteObject(objectName: string): Promise<void> {
    if (!s3Client) {
      throw new Error("S3 client is not available");
    }

    const command = new DeleteObjectCommand({
      Bucket: bucketName,
      Key: objectName,
    });

    await s3Client.send(command);
  }
}
