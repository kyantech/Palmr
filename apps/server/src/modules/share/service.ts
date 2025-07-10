import bcrypt from "bcryptjs";

import { prisma } from "../../shared/prisma";
import { EmailService } from "../email/service";
import { CreateShareInput, ShareResponseSchema, UpdateShareInput } from "./dto";
import { IShareRepository, PrismaShareRepository } from "./repository";

export class ShareService {
  constructor(private readonly shareRepository: IShareRepository = new PrismaShareRepository()) {}

  private emailService = new EmailService();

  private formatShareResponse(share: any) {
    return {
      ...share,
      createdAt: share.createdAt.toISOString(),
      updatedAt: share.updatedAt.toISOString(),
      expiration: share.expiration?.toISOString() || null,
      alias: share.alias
        ? {
            ...share.alias,
            createdAt: share.alias.createdAt.toISOString(),
            updatedAt: share.alias.updatedAt.toISOString(),
          }
        : null,
      security: {
        maxViews: share.security.maxViews,
        hasPassword: !!share.security.password,
      },
      files:
        share.files?.map((file: any) => ({
          ...file,
          size: file.size.toString(),
          createdAt: file.createdAt.toISOString(),
          updatedAt: file.updatedAt.toISOString(),
        })) || [],
      recipients:
        share.recipients?.map((recipient: any) => ({
          ...recipient,
          createdAt: recipient.createdAt.toISOString(),
          updatedAt: recipient.updatedAt.toISOString(),
        })) || [],
    };
  }

  async createShare(data: CreateShareInput, userId: string) {
    const { password, maxViews, ...shareData } = data;

    const security = await prisma.shareSecurity.create({
      data: {
        password: password ? await bcrypt.hash(password, 10) : null,
        maxViews: maxViews,
      },
    });

    const share = await this.shareRepository.createShare({
      ...shareData,
      securityId: security.id,
      creatorId: userId,
    });

    const shareWithRelations = await this.shareRepository.findShareById(share.id);
    return ShareResponseSchema.parse(this.formatShareResponse(shareWithRelations));
  }

  async getShare(shareId: string, password?: string, userId?: string) {
    const share = await this.shareRepository.findShareById(shareId);

    if (!share) {
      throw new Error("Share not found");
    }

    if (userId && share.creatorId === userId) {
      return ShareResponseSchema.parse(this.formatShareResponse(share));
    }

    if (share.expiration && new Date() > new Date(share.expiration)) {
      throw new Error("Share has expired");
    }

    if (share.security?.maxViews && share.views >= share.security.maxViews) {
      throw new Error("Share has reached maximum views");
    }

    if (share.security?.password && !password) {
      throw new Error("Password required");
    }

    if (share.security?.password && password) {
      const isPasswordValid = await bcrypt.compare(password, share.security.password);
      if (!isPasswordValid) {
        throw new Error("Invalid password");
      }
    }

    await this.shareRepository.incrementViews(shareId);

    const updatedShare = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(this.formatShareResponse(updatedShare));
  }

  async updateShare(shareId: string, data: Omit<UpdateShareInput, "id">, userId: string) {
    const { password, maxViews, recipients, ...shareData } = data;

    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    if (password || maxViews !== undefined) {
      await this.shareRepository.updateShareSecurity(share.securityId, {
        password: password ? await bcrypt.hash(password, 10) : undefined,
        maxViews: maxViews,
      });
    }

    if (recipients) {
      await this.shareRepository.removeRecipients(
        shareId,
        share.recipients.map((r) => r.email)
      );
      if (recipients.length > 0) {
        await this.shareRepository.addRecipients(shareId, recipients);
      }
    }

    await this.shareRepository.updateShare(shareId, {
      ...shareData,
      expiration: shareData.expiration ? new Date(shareData.expiration) : null,
    });
    const shareWithRelations = await this.shareRepository.findShareById(shareId);

    return this.formatShareResponse(shareWithRelations);
  }

  async deleteShare(id: string) {
    const share = await this.shareRepository.findShareById(id);
    if (!share) {
      throw new Error("Share not found");
    }

    const deleted = await prisma.$transaction(async (tx) => {
      await tx.share.update({
        where: { id },
        data: {
          files: {
            set: [],
          },
        },
      });

      const deletedShare = await tx.share.delete({
        where: { id },
        include: {
          security: true,
          files: true,
        },
      });

      if (deletedShare.security) {
        await tx.shareSecurity.delete({
          where: { id: deletedShare.security.id },
        });
      }

      return deletedShare;
    });

    return ShareResponseSchema.parse(this.formatShareResponse(deleted));
  }

  async listUserShares(userId: string) {
    const shares = await this.shareRepository.findSharesByUserId(userId);
    return shares.map((share) => this.formatShareResponse(share));
  }

  async updateSharePassword(shareId: string, userId: string, password: string | null) {
    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    await this.shareRepository.updateShareSecurity(share.security.id, {
      password: password ? await bcrypt.hash(password, 10) : null,
    });

    const updated = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(this.formatShareResponse(updated));
  }

  async addFilesToShare(shareId: string, userId: string, fileIds: string[]) {
    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    const existingFiles = await this.shareRepository.findFilesByIds(fileIds);
    const notFoundFiles = fileIds.filter((id) => !existingFiles.some((file) => file.id === id));

    if (notFoundFiles.length > 0) {
      throw new Error(`Files not found: ${notFoundFiles.join(", ")}`);
    }

    await this.shareRepository.addFilesToShare(shareId, fileIds);
    const updated = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(this.formatShareResponse(updated));
  }

  async removeFilesFromShare(shareId: string, userId: string, fileIds: string[]) {
    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    await this.shareRepository.removeFilesFromShare(shareId, fileIds);
    const updated = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(this.formatShareResponse(updated));
  }

  async findShareById(id: string) {
    const share = await this.shareRepository.findShareById(id);
    if (!share) {
      throw new Error("Share not found");
    }
    return share;
  }

  async addRecipients(shareId: string, userId: string, emails: string[]) {
    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    await this.shareRepository.addRecipients(shareId, emails);
    const updated = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(this.formatShareResponse(updated));
  }

  async removeRecipients(shareId: string, userId: string, emails: string[]) {
    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    await this.shareRepository.removeRecipients(shareId, emails);
    const updated = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(this.formatShareResponse(updated));
  }

  async createOrUpdateAlias(shareId: string, alias: string, userId: string) {
    const share = await this.findShareById(shareId);

    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    const existingAlias = await prisma.shareAlias.findUnique({
      where: { alias },
    });

    if (existingAlias && existingAlias.shareId !== shareId) {
      throw new Error("Alias already in use");
    }

    const shareAlias = await prisma.shareAlias.upsert({
      where: { shareId },
      create: { shareId, alias },
      update: { alias },
    });

    return {
      ...shareAlias,
      createdAt: shareAlias.createdAt.toISOString(),
      updatedAt: shareAlias.updatedAt.toISOString(),
    };
  }

  async getShareByAlias(alias: string, password?: string) {
    const shareAlias = await prisma.shareAlias.findUnique({
      where: { alias },
      include: {
        share: {
          include: {
            security: true,
            files: true,
            recipients: true,
          },
        },
      },
    });

    if (!shareAlias) {
      throw new Error("Share not found");
    }

    return this.getShare(shareAlias.shareId, password);
  }

  async notifyRecipients(shareId: string, userId: string, shareLink: string) {
    const share = await this.shareRepository.findShareById(shareId);

    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to access this share");
    }

    if (!share.recipients || share.recipients.length === 0) {
      throw new Error("No recipients found for this share");
    }

    const notifiedRecipients: string[] = [];

    for (const recipient of share.recipients) {
      try {
        await this.emailService.sendShareNotification(recipient.email, shareLink, share.name || undefined);
        notifiedRecipients.push(recipient.email);
      } catch (error) {
        console.error(`Failed to send email to ${recipient.email}:`, error);
      }
    }

    return {
      message: `Successfully sent notifications to ${notifiedRecipients.length} recipients`,
      notifiedRecipients,
    };
  }
}
