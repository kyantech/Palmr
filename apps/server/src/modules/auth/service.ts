import crypto from "node:crypto";
import bcrypt from "bcryptjs";

import { prisma } from "../../shared/prisma";
import { ConfigService } from "../config/service";
import { EmailService } from "../email/service";
import { TwoFactorService } from "../two-factor/service";
import { UserResponseSchema } from "../user/dto";
import { PrismaUserRepository } from "../user/repository";
import { LoginInput } from "./dto";
import { TrustedDeviceService } from "./trusted-device.service";

export class AuthService {
  private userRepository = new PrismaUserRepository();
  private configService = new ConfigService();
  private emailService = new EmailService();
  private twoFactorService = new TwoFactorService();
  private trustedDeviceService = new TrustedDeviceService();

  async login(data: LoginInput, userAgent?: string, ipAddress?: string) {
    const passwordAuthEnabled = await this.configService.getValue("passwordAuthEnabled");
    if (passwordAuthEnabled === "false") {
      throw new Error("Password authentication is disabled. Please use an external authentication provider.");
    }

    const user = await this.userRepository.findUserByEmailOrUsername(data.emailOrUsername);
    if (!user) {
      throw new Error("Invalid credentials");
    }

    if (!user.isActive) {
      throw new Error("Account is inactive. Please contact an administrator.");
    }

    const maxAttempts = Number(await this.configService.getValue("maxLoginAttempts"));
    const blockDurationSeconds = Number(await this.configService.getValue("loginBlockDuration"));
    const blockDuration = blockDurationSeconds * 1000;

    const loginAttempt = await prisma.loginAttempt.findUnique({
      where: { userId: user.id },
    });

    if (loginAttempt) {
      if (loginAttempt.attempts >= maxAttempts && Date.now() - loginAttempt.lastAttempt.getTime() < blockDuration) {
        const remainingTime = Math.ceil(
          (blockDuration - (Date.now() - loginAttempt.lastAttempt.getTime())) / 1000 / 60
        );
        throw new Error(`Too many failed attempts. Please try again in ${remainingTime} minutes.`);
      }

      if (Date.now() - loginAttempt.lastAttempt.getTime() >= blockDuration) {
        await prisma.loginAttempt.delete({
          where: { userId: user.id },
        });
      }
    }

    if (!user.password) {
      throw new Error("This account uses external authentication. Please use the appropriate login method.");
    }

    const isValid = await bcrypt.compare(data.password, user.password);

    if (!isValid) {
      await prisma.loginAttempt.upsert({
        where: { userId: user.id },
        create: {
          userId: user.id,
          attempts: 1,
          lastAttempt: new Date(),
        },
        update: {
          attempts: {
            increment: 1,
          },
          lastAttempt: new Date(),
        },
      });

      throw new Error("Invalid credentials");
    }

    if (loginAttempt) {
      await prisma.loginAttempt.delete({
        where: { userId: user.id },
      });
    }

    const has2FA = await this.twoFactorService.isEnabled(user.id);

    if (has2FA) {
      if (userAgent && ipAddress) {
        const isDeviceTrusted = await this.trustedDeviceService.isDeviceTrusted(user.id, userAgent, ipAddress);
        if (isDeviceTrusted) {
          // Update last used timestamp for trusted device
          await this.trustedDeviceService.updateLastUsed(user.id, userAgent, ipAddress);
          return UserResponseSchema.parse(user);
        }
      }

      return {
        requiresTwoFactor: true,
        userId: user.id,
        message: "Two-factor authentication required",
      };
    }

    return UserResponseSchema.parse(user);
  }

  async completeTwoFactorLogin(
    userId: string,
    token: string,
    rememberDevice: boolean = false,
    userAgent?: string,
    ipAddress?: string
  ) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
    });

    if (!user) {
      throw new Error("User not found");
    }

    if (!user.isActive) {
      throw new Error("Account is inactive. Please contact an administrator.");
    }

    const verificationResult = await this.twoFactorService.verifyToken(userId, token);

    if (!verificationResult.success) {
      throw new Error("Invalid two-factor authentication code");
    }

    await prisma.loginAttempt.deleteMany({
      where: { userId },
    });

    if (rememberDevice && userAgent && ipAddress) {
      await this.trustedDeviceService.addTrustedDevice(userId, userAgent, ipAddress);
    } else if (userAgent && ipAddress) {
      // Update last used timestamp if this is already a trusted device
      const isDeviceTrusted = await this.trustedDeviceService.isDeviceTrusted(userId, userAgent, ipAddress);
      if (isDeviceTrusted) {
        await this.trustedDeviceService.updateLastUsed(userId, userAgent, ipAddress);
      }
    }

    return UserResponseSchema.parse(user);
  }

  async requestPasswordReset(email: string, origin: string) {
    const passwordAuthEnabled = await this.configService.getValue("passwordAuthEnabled");
    if (passwordAuthEnabled === "false") {
      throw new Error("Password authentication is disabled. Password reset is not available.");
    }

    const user = await this.userRepository.findUserByEmail(email);
    if (!user) {
      return;
    }

    const token = crypto.randomBytes(128).toString("hex");
    const expirationSeconds = Number(await this.configService.getValue("passwordResetTokenExpiration"));

    await prisma.passwordReset.create({
      data: {
        userId: user.id,
        token,
        expiresAt: new Date(Date.now() + expirationSeconds * 1000),
      },
    });

    try {
      await this.emailService.sendPasswordResetEmail(email, token, origin);
    } catch (error) {
      console.error("Failed to send password reset email:", error);
      throw new Error("Failed to send password reset email");
    }
  }

  async resetPassword(token: string, newPassword: string) {
    const passwordAuthEnabled = await this.configService.getValue("passwordAuthEnabled");
    if (passwordAuthEnabled === "false") {
      throw new Error("Password authentication is disabled. Password reset is not available.");
    }

    const resetRequest = await prisma.passwordReset.findFirst({
      where: {
        token,
        used: false,
        expiresAt: {
          gt: new Date(),
        },
      },
      include: {
        user: true,
      },
    });

    if (!resetRequest) {
      throw new Error("Invalid or expired reset token");
    }

    const hashedPassword = await bcrypt.hash(newPassword, 10);

    await prisma.$transaction([
      prisma.user.update({
        where: { id: resetRequest.userId },
        data: { password: hashedPassword },
      }),
      prisma.passwordReset.update({
        where: { id: resetRequest.id },
        data: { used: true },
      }),
    ]);
  }

  async getUserById(userId: string) {
    const user = await prisma.user.findUnique({
      where: { id: userId },
    });
    if (!user) {
      throw new Error("User not found");
    }
    return UserResponseSchema.parse(user);
  }

  async getTrustedDevices(userId: string) {
    return await this.trustedDeviceService.getUserTrustedDevices(userId);
  }

  async removeTrustedDevice(userId: string, deviceId: string) {
    return await this.trustedDeviceService.removeTrustedDevice(userId, deviceId);
  }

  async removeAllTrustedDevices(userId: string) {
    const result = await this.trustedDeviceService.removeAllTrustedDevices(userId);
    return {
      success: true,
      message: "All trusted devices removed successfully",
      removedCount: result.count,
    };
  }
}
