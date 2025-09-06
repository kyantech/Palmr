import bcrypt from "bcryptjs";

import { prisma } from "../../shared/prisma";
import { EmailService } from "../email/service";
import { FolderService } from "../folder/service";
import { UserService } from "../user/service";
import { CreateShareInput, ShareResponseSchema, UpdateShareInput } from "./dto";
import { IShareRepository, PrismaShareRepository } from "./repository";

export class ShareService {
  constructor(private readonly shareRepository: IShareRepository = new PrismaShareRepository()) {}

  private emailService = new EmailService();
  private userService = new UserService();
  private folderService = new FolderService();

  private async formatShareResponse(share: any) {
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
      folders:
        share.folders && share.folders.length > 0
          ? await Promise.all(
              share.folders.map(async (folder: any) => {
                const totalSize = await this.folderService.calculateFolderSize(folder.id, folder.userId);
                return {
                  ...folder,
                  totalSize: totalSize.toString(),
                  createdAt: folder.createdAt.toISOString(),
                  updatedAt: folder.updatedAt.toISOString(),
                };
              })
            )
          : [],
      recipients:
        share.recipients?.map((recipient: any) => ({
          ...recipient,
          createdAt: recipient.createdAt.toISOString(),
          updatedAt: recipient.updatedAt.toISOString(),
        })) || [],
    };
  }

  async createShare(data: CreateShareInput, userId: string) {
    const { password, maxViews, files, folders, ...shareData } = data;

    // Validate files belong to user if provided
    if (files && files.length > 0) {
      const existingFiles = await prisma.file.findMany({
        where: {
          id: { in: files },
          userId: userId,
        },
      });
      const notFoundFiles = files.filter((id) => !existingFiles.some((file) => file.id === id));
      if (notFoundFiles.length > 0) {
        throw new Error(`Files not found or access denied: ${notFoundFiles.join(", ")}`);
      }
    }

    // Validate folders belong to user if provided
    if (folders && folders.length > 0) {
      const existingFolders = await prisma.folder.findMany({
        where: {
          id: { in: folders },
          userId: userId,
        },
      });
      const notFoundFolders = folders.filter((id) => !existingFolders.some((folder) => folder.id === id));
      if (notFoundFolders.length > 0) {
        throw new Error(`Folders not found or access denied: ${notFoundFolders.join(", ")}`);
      }
    }

    // Require at least one file or folder
    if ((!files || files.length === 0) && (!folders || folders.length === 0)) {
      throw new Error("At least one file or folder must be selected to create a share");
    }

    const security = await prisma.shareSecurity.create({
      data: {
        password: password ? await bcrypt.hash(password, 10) : null,
        maxViews: maxViews,
      },
    });

    const share = await this.shareRepository.createShare({
      ...shareData,
      files,
      folders,
      securityId: security.id,
      creatorId: userId,
    });

    const shareWithRelations = await this.shareRepository.findShareById(share.id);
    return ShareResponseSchema.parse(await this.formatShareResponse(shareWithRelations));
  }

  async getShare(shareId: string, password?: string, userId?: string) {
    const share = await this.shareRepository.findShareById(shareId);

    if (!share) {
      throw new Error("Share not found");
    }

    if (userId && share.creatorId === userId) {
      return ShareResponseSchema.parse(await this.formatShareResponse(share));
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
    return ShareResponseSchema.parse(await this.formatShareResponse(updatedShare));
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

    return await this.formatShareResponse(shareWithRelations);
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

    return ShareResponseSchema.parse(await this.formatShareResponse(deleted));
  }

  async listUserShares(userId: string) {
    const shares = await this.shareRepository.findSharesByUserId(userId);
    return await Promise.all(shares.map(async (share) => await this.formatShareResponse(share)));
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
    return ShareResponseSchema.parse(await this.formatShareResponse(updated));
  }

  async addItemsToShare(shareId: string, userId: string, fileIds: string[], folderIds: string[]) {
    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    // Validate files if provided
    if (fileIds.length > 0) {
      const existingFiles = await this.shareRepository.findFilesByIds(fileIds);
      const notFoundFiles = fileIds.filter((id) => !existingFiles.some((file) => file.id === id));

      if (notFoundFiles.length > 0) {
        throw new Error(`Files not found: ${notFoundFiles.join(", ")}`);
      }

      await this.shareRepository.addFilesToShare(shareId, fileIds);
    }

    // Validate folders if provided
    if (folderIds.length > 0) {
      const existingFolders = await this.shareRepository.findFoldersByIds(folderIds);
      const notFoundFolders = folderIds.filter((id) => !existingFolders.some((folder) => folder.id === id));

      if (notFoundFolders.length > 0) {
        throw new Error(`Folders not found: ${notFoundFolders.join(", ")}`);
      }

      await this.shareRepository.addFoldersToShare(shareId, folderIds);
    }

    const updated = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(await this.formatShareResponse(updated));
  }

  async removeItemsFromShare(shareId: string, userId: string, fileIds: string[], folderIds: string[]) {
    const share = await this.shareRepository.findShareById(shareId);
    if (!share) {
      throw new Error("Share not found");
    }

    if (share.creatorId !== userId) {
      throw new Error("Unauthorized to update this share");
    }

    // Remove files if provided
    if (fileIds.length > 0) {
      await this.shareRepository.removeFilesFromShare(shareId, fileIds);
    }

    // Remove folders if provided
    if (folderIds.length > 0) {
      await this.shareRepository.removeFoldersFromShare(shareId, folderIds);
    }

    const updated = await this.shareRepository.findShareById(shareId);
    return ShareResponseSchema.parse(await this.formatShareResponse(updated));
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
    return ShareResponseSchema.parse(await this.formatShareResponse(updated));
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
    return ShareResponseSchema.parse(await this.formatShareResponse(updated));
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

    // Get sender information
    let senderName = "Someone";
    try {
      const sender = await this.userService.getUserById(userId);
      if (sender.firstName && sender.lastName) {
        senderName = `${sender.firstName} ${sender.lastName}`;
      } else if (sender.firstName) {
        senderName = sender.firstName;
      } else if (sender.username) {
        senderName = sender.username;
      }
    } catch (error) {
      console.error(`Failed to get sender information for user ${userId}:`, error);
    }

    const notifiedRecipients: string[] = [];

    for (const recipient of share.recipients) {
      try {
        await this.emailService.sendShareNotification(recipient.email, shareLink, share.name || undefined, senderName);
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
