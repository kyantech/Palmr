import crypto from "node:crypto";
import bcrypt from "bcryptjs";
import QRCode from "qrcode";
import speakeasy from "speakeasy";

import { prisma } from "../../shared/prisma";
import { ConfigService } from "../config/service";

interface BackupCode {
  code: string;
  used: boolean;
}

export class TwoFactorService {
  private configService = new ConfigService();

  /**
   * Generate a new 2FA secret and QR code for setup
   */
  async generateSetup(userId: string, userEmail: string, appName?: string) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
      select: { id: true, email: true, twoFactorEnabled: true },
    });

    if (!user) {
      throw new Error("User not found");
    }

    if (user.twoFactorEnabled) {
      throw new Error("Two-factor authentication is already enabled");
    }

    const secret = speakeasy.generateSecret({
      name: `${appName || "Palmr"}:${userEmail}`,
      issuer: appName || "Palmr",
      length: 32,
    });

    const qrCodeUrl = await QRCode.toDataURL(secret.otpauth_url || "");

    return {
      secret: secret.base32,
      qrCode: qrCodeUrl,
      manualEntryKey: secret.base32,
      backupCodes: await this.generateBackupCodes(),
    };
  }

  /**
   * Verify setup token and enable 2FA
   */
  async verifySetup(userId: string, token: string, secret: string) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
      select: { id: true, twoFactorEnabled: true },
    });

    if (!user) {
      throw new Error("User not found");
    }

    if (user.twoFactorEnabled) {
      throw new Error("Two-factor authentication is already enabled");
    }

    const verified = speakeasy.totp.verify({
      secret: secret,
      encoding: "base32",
      token: token,
      window: 1,
    });

    if (!verified) {
      throw new Error("Invalid verification code");
    }

    const backupCodes = await this.generateBackupCodes();

    await prisma.user.update({
      where: { id: userId },
      data: {
        twoFactorEnabled: true,
        twoFactorSecret: secret,
        twoFactorBackupCodes: JSON.stringify(backupCodes),
        twoFactorVerified: true,
      },
    });

    return {
      success: true,
      backupCodes: backupCodes.map((bc) => bc.code),
    };
  }

  /**
   * Verify a 2FA token during login
   */
  async verifyToken(userId: string, token: string) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
      select: {
        id: true,
        twoFactorEnabled: true,
        twoFactorSecret: true,
        twoFactorBackupCodes: true,
      },
    });

    if (!user) {
      throw new Error("User not found");
    }

    if (!user.twoFactorEnabled || !user.twoFactorSecret) {
      throw new Error("Two-factor authentication is not enabled");
    }

    const verified = speakeasy.totp.verify({
      secret: user.twoFactorSecret,
      encoding: "base32",
      token: token,
      window: 1,
    });

    if (verified) {
      return { success: true, method: "totp" };
    }

    if (user.twoFactorBackupCodes) {
      const backupCodes: BackupCode[] = JSON.parse(user.twoFactorBackupCodes);
      const backupCodeIndex = backupCodes.findIndex((bc) => bc.code === token && !bc.used);

      if (backupCodeIndex !== -1) {
        backupCodes[backupCodeIndex].used = true;

        await prisma.user.update({
          where: { id: userId },
          data: {
            twoFactorBackupCodes: JSON.stringify(backupCodes),
          },
        });

        return { success: true, method: "backup" };
      }
    }

    throw new Error("Invalid verification code");
  }

  /**
   * Disable 2FA for a user
   */
  async disable2FA(userId: string, password: string) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
      select: {
        id: true,
        password: true,
        twoFactorEnabled: true,
      },
    });

    if (!user) {
      throw new Error("User not found");
    }

    if (!user.twoFactorEnabled) {
      throw new Error("Two-factor authentication is not enabled");
    }

    if (!user.password) {
      throw new Error("Password verification required");
    }

    let isValidPassword = false;
    try {
      isValidPassword = await bcrypt.compare(password, user.password);
    } catch (error) {
      console.error("bcrypt.compare error:", error);
      throw new Error("Password verification failed");
    }
    if (!isValidPassword) {
      throw new Error("Invalid password");
    }

    await prisma.user.update({
      where: { id: userId },
      data: {
        twoFactorEnabled: false,
        twoFactorSecret: null,
        twoFactorBackupCodes: null,
        twoFactorVerified: false,
      },
    });

    return { success: true };
  }

  /**
   * Generate new backup codes
   */
  async generateNewBackupCodes(userId: string) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
      select: { id: true, twoFactorEnabled: true },
    });

    if (!user) {
      throw new Error("User not found");
    }

    if (!user.twoFactorEnabled) {
      throw new Error("Two-factor authentication is not enabled");
    }

    const backupCodes = await this.generateBackupCodes();

    await prisma.user.update({
      where: { id: userId },
      data: {
        twoFactorBackupCodes: JSON.stringify(backupCodes),
      },
    });

    return backupCodes.map((bc) => bc.code);
  }

  /**
   * Get 2FA status for a user
   */
  async getStatus(userId: string) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
      select: {
        id: true,
        twoFactorEnabled: true,
        twoFactorVerified: true,
        twoFactorBackupCodes: true,
      },
    });

    if (!user) {
      throw new Error("User not found");
    }

    let availableBackupCodes = 0;
    if (user.twoFactorBackupCodes) {
      const backupCodes: BackupCode[] = JSON.parse(user.twoFactorBackupCodes);
      availableBackupCodes = backupCodes.filter((bc) => !bc.used).length;
    }

    return {
      enabled: user.twoFactorEnabled,
      verified: user.twoFactorVerified,
      availableBackupCodes,
    };
  }

  /**
   * Generate backup codes
   */
  private async generateBackupCodes(): Promise<BackupCode[]> {
    const codes: BackupCode[] = [];

    for (let i = 0; i < 10; i++) {
      const code = crypto.randomBytes(4).toString("hex").toUpperCase();
      codes.push({
        code: code.match(/.{1,4}/g)?.join("-") || code,
        used: false,
      });
    }

    return codes;
  }

  /**
   * Check if user has 2FA enabled
   */
  async isEnabled(userId: string): Promise<boolean> {
    const user = await prisma.user.findUnique({
      where: { id: userId },
      select: { twoFactorEnabled: true },
    });

    return user?.twoFactorEnabled ?? false;
  }
}
