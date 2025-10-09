import sharp from "sharp";

import { prisma } from "../../shared/prisma";

export class AvatarService {
  async uploadAvatar(buffer: Buffer): Promise<string> {
    try {
      const metadata = await sharp(buffer, {
        animated: false,
        limitInputPixels: 268402689,
      }).metadata();

      if (!metadata.width || !metadata.height) {
        throw new Error("Invalid image file");
      }

      if (metadata.format === "pdf" || metadata.format === "svg") {
        throw new Error("PDF and SVG files are not supported for avatars");
      }

      const webpBuffer = await sharp(buffer, {
        animated: false,
        limitInputPixels: 268402689,
      })
        .resize(100, 100, {
          fit: "cover",
          background: { r: 255, g: 255, b: 255, alpha: 0 },
        })
        .webp({
          quality: 60,
          effort: 6,
          nearLossless: true,
          alphaQuality: 100,
          lossless: true,
        })
        .toBuffer();

      return `data:image/webp;base64,${webpBuffer.toString("base64")}`;
    } catch (error: any) {
      console.error("Error processing avatar:", error);

      if (error.message?.includes("PDF") || error.message?.includes("SVG")) {
        throw new Error("Unsupported file format for avatar. Please use JPG, PNG, or GIF images.");
      }

      if (error.message?.includes("Input buffer") || error.message?.includes("unsupported")) {
        throw new Error(
          "Unable to process this image file. Please try a different image or convert it to a standard format (JPG/PNG)."
        );
      }

      throw new Error("Failed to process avatar image. Please ensure the file is a valid image.");
    }
  }

  async deleteAvatar(userId: string): Promise<void> {
    try {
      await prisma.user.update({
        where: { id: userId },
        data: {
          image: null,
          updatedAt: new Date(),
        },
      });
    } catch (error) {
      console.error("Error deleting avatar from database:", error);
      throw error;
    }
  }
}
