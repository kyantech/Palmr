import sharp from "sharp";

import { prisma } from "../../shared/prisma";

export class LogoService {
  async uploadLogo(buffer: Buffer): Promise<string> {
    try {
      const metadata = await sharp(buffer).metadata();
      if (!metadata.width || !metadata.height) {
        throw new Error("Invalid image file");
      }

      const webpBuffer = await sharp(buffer)
        .resize(100, 100, {
          fit: "contain",
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
    } catch (error) {
      console.error("Error processing logo:", error);
      throw error;
    }
  }

  async deleteLogo(): Promise<void> {
    try {
      await prisma.appConfig.update({
        where: { key: "appLogo" },
        data: {
          value: "",
          updatedAt: new Date(),
        },
      });
    } catch (error) {
      console.error("Error deleting logo from database:", error);
      throw error;
    }
  }
}
