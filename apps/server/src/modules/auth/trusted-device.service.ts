import crypto from "node:crypto";

import { prisma } from "../../shared/prisma";

export class TrustedDeviceService {
  private generateDeviceHash(userAgent: string, ipAddress: string): string {
    const deviceInfo = `${userAgent}-${ipAddress}`;
    return crypto.createHash("sha256").update(deviceInfo).digest("hex");
  }

  async isDeviceTrusted(userId: string, userAgent: string, ipAddress: string): Promise<boolean> {
    const deviceHash = this.generateDeviceHash(userAgent, ipAddress);

    const trustedDevice = await prisma.trustedDevice.findFirst({
      where: {
        userId,
        deviceHash,
        expiresAt: {
          gt: new Date(),
        },
      },
    });

    return !!trustedDevice;
  }

  async addTrustedDevice(userId: string, userAgent: string, ipAddress: string, deviceName?: string): Promise<void> {
    const deviceHash = this.generateDeviceHash(userAgent, ipAddress);
    const expiresAt = new Date();
    expiresAt.setDate(expiresAt.getDate() + 30); // 30 dias

    await prisma.trustedDevice.upsert({
      where: {
        deviceHash,
      },
      create: {
        userId,
        deviceHash,
        deviceName,
        userAgent,
        ipAddress,
        expiresAt,
        lastUsedAt: new Date(),
      },
      update: {
        expiresAt,
        userAgent,
        ipAddress,
        lastUsedAt: new Date(),
      },
    });
  }

  async cleanupExpiredDevices(): Promise<void> {
    await prisma.trustedDevice.deleteMany({
      where: {
        expiresAt: {
          lt: new Date(),
        },
      },
    });
  }

  async getUserTrustedDevices(userId: string) {
    return prisma.trustedDevice.findMany({
      where: {
        userId,
        expiresAt: {
          gt: new Date(),
        },
      },
      orderBy: {
        createdAt: "desc",
      },
    });
  }

  async removeTrustedDevice(userId: string, deviceId: string): Promise<void> {
    await prisma.trustedDevice.deleteMany({
      where: {
        id: deviceId,
        userId,
      },
    });
  }

  async removeAllTrustedDevices(userId: string): Promise<{ count: number }> {
    const result = await prisma.trustedDevice.deleteMany({
      where: {
        userId,
      },
    });
    return { count: result.count };
  }

  async updateLastUsed(userId: string, userAgent: string, ipAddress: string): Promise<void> {
    const deviceHash = this.generateDeviceHash(userAgent, ipAddress);

    await prisma.trustedDevice.updateMany({
      where: {
        userId,
        deviceHash,
      },
      data: {
        lastUsedAt: new Date(),
      },
    });
  }
}
