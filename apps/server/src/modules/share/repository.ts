import type { Share, ShareSecurity } from "@prisma/client";

import { prisma } from "../../shared/prisma";
import type { CreateShareInput } from "./dto";

export interface IShareRepository {
  createShare(data: CreateShareInput & { securityId: string; creatorId: string }): Promise<Share>;
  findShareById(id: string): Promise<
    | (Share & {
        security: ShareSecurity;
        files: any[];
        folders: any[];
        recipients: { email: string }[];
      })
    | null
  >;
  findShareBySecurityId(
    securityId: string
  ): Promise<(Share & { security: ShareSecurity; files: any[]; folders: any[] }) | null>;
  findShareByAlias(
    alias: string
  ): Promise<(Share & { security: ShareSecurity; files: any[]; folders: any[]; recipients: any[] }) | null>;
  updateShare(id: string, data: Partial<Share>): Promise<Share>;
  updateShareSecurity(id: string, data: Partial<ShareSecurity>): Promise<ShareSecurity>;
  deleteShare(id: string): Promise<Share>;
  incrementViews(id: string): Promise<Share>;
  addFilesToShare(shareId: string, fileIds: string[]): Promise<void>;
  removeFilesFromShare(shareId: string, fileIds: string[]): Promise<void>;
  addFoldersToShare(shareId: string, folderIds: string[]): Promise<void>;
  removeFoldersFromShare(shareId: string, folderIds: string[]): Promise<void>;
  findFilesByIds(fileIds: string[]): Promise<any[]>;
  findFoldersByIds(folderIds: string[]): Promise<any[]>;
  addRecipients(shareId: string, emails: string[]): Promise<void>;
  removeRecipients(shareId: string, emails: string[]): Promise<void>;
  findSharesByUserId(
    userId: string
  ): Promise<(Share & { security: ShareSecurity; files: any[]; folders: any[]; recipients: any[]; alias: any })[]>;
}

export class PrismaShareRepository implements IShareRepository {
  async createShare(
    data: Omit<CreateShareInput, "password" | "maxViews"> & { securityId: string; creatorId: string }
  ): Promise<Share> {
    const { files, folders, recipients, expiration, ...shareData } = data;

    const validFiles = (files ?? []).filter((id) => id && id.trim().length > 0);
    const validFolders = (folders ?? []).filter((id) => id && id.trim().length > 0);
    const validRecipients = (recipients ?? []).filter((email) => email && email.trim().length > 0);

    return prisma.share.create({
      data: {
        ...shareData,
        expiration: expiration ? new Date(expiration) : null,
        files:
          validFiles.length > 0
            ? {
                connect: validFiles.map((id) => ({ id })),
              }
            : undefined,
        folders:
          validFolders.length > 0
            ? {
                connect: validFolders.map((id) => ({ id })),
              }
            : undefined,
        recipients:
          validRecipients?.length > 0
            ? {
                create: validRecipients.map((email) => ({
                  email: email.trim().toLowerCase(),
                })),
              }
            : undefined,
      },
    });
  }

  async findShareById(id: string) {
    return prisma.share.findUnique({
      where: { id },
      include: {
        alias: true,
        security: true,
        files: true,
        folders: {
          select: {
            id: true,
            name: true,
            description: true,
            objectName: true,
            parentId: true,
            userId: true,
            createdAt: true,
            updatedAt: true,
            _count: {
              select: {
                files: true,
                children: true,
              },
            },
          },
        },
        recipients: true,
      },
    });
  }

  async findShareBySecurityId(securityId: string) {
    return prisma.share.findUnique({
      where: { securityId },
      include: {
        security: true,
        files: true,
        folders: {
          select: {
            id: true,
            name: true,
            description: true,
            objectName: true,
            parentId: true,
            userId: true,
            createdAt: true,
            updatedAt: true,
            _count: {
              select: {
                files: true,
                children: true,
              },
            },
          },
        },
      },
    });
  }

  async findShareByAlias(alias: string) {
    const shareAlias = await prisma.shareAlias.findUnique({
      where: { alias },
      include: {
        share: {
          include: {
            security: true,
            files: true,
            folders: {
              select: {
                id: true,
                name: true,
                description: true,
                objectName: true,
                parentId: true,
                userId: true,
                createdAt: true,
                updatedAt: true,
                _count: {
                  select: {
                    files: true,
                    children: true,
                  },
                },
              },
            },
            recipients: true,
          },
        },
      },
    });

    return shareAlias?.share || null;
  }

  async updateShare(id: string, data: Partial<Share>): Promise<Share> {
    return prisma.share.update({
      where: { id },
      data,
    });
  }

  async updateShareSecurity(id: string, data: Partial<ShareSecurity>): Promise<ShareSecurity> {
    return prisma.shareSecurity.update({
      where: { id },
      data,
    });
  }

  async deleteShare(id: string): Promise<Share> {
    return prisma.share.delete({
      where: { id },
    });
  }

  async incrementViews(id: string): Promise<Share> {
    return prisma.share.update({
      where: { id },
      data: {
        views: {
          increment: 1,
        },
      },
    });
  }

  async addFilesToShare(shareId: string, fileIds: string[]): Promise<void> {
    await prisma.share.update({
      where: { id: shareId },
      data: {
        files: {
          connect: fileIds.map((id) => ({ id })),
        },
      },
    });
  }

  async addFoldersToShare(shareId: string, folderIds: string[]): Promise<void> {
    await prisma.share.update({
      where: { id: shareId },
      data: {
        folders: {
          connect: folderIds.map((id) => ({ id })),
        },
      },
    });
  }

  async removeFilesFromShare(shareId: string, fileIds: string[]): Promise<void> {
    await prisma.share.update({
      where: { id: shareId },
      data: {
        files: {
          disconnect: fileIds.map((id) => ({ id })),
        },
      },
    });
  }

  async removeFoldersFromShare(shareId: string, folderIds: string[]): Promise<void> {
    await prisma.share.update({
      where: { id: shareId },
      data: {
        folders: {
          disconnect: folderIds.map((id) => ({ id })),
        },
      },
    });
  }

  async findFilesByIds(fileIds: string[]): Promise<any[]> {
    return prisma.file.findMany({
      where: {
        id: {
          in: fileIds,
        },
      },
    });
  }

  async findFoldersByIds(folderIds: string[]): Promise<any[]> {
    return prisma.folder.findMany({
      where: {
        id: {
          in: folderIds,
        },
      },
    });
  }

  async addRecipients(shareId: string, emails: string[]): Promise<void> {
    await prisma.share.update({
      where: { id: shareId },
      data: {
        recipients: {
          create: emails.map((email) => ({
            email,
          })),
        },
      },
    });
  }

  async removeRecipients(shareId: string, emails: string[]): Promise<void> {
    await prisma.share.update({
      where: { id: shareId },
      data: {
        recipients: {
          deleteMany: {
            email: {
              in: emails,
            },
          },
        },
      },
    });
  }

  async findSharesByUserId(userId: string) {
    return prisma.share.findMany({
      where: {
        creatorId: userId,
      },
      include: {
        security: true,
        files: true,
        folders: {
          select: {
            id: true,
            name: true,
            description: true,
            objectName: true,
            parentId: true,
            userId: true,
            createdAt: true,
            updatedAt: true,
            _count: {
              select: {
                files: true,
                children: true,
              },
            },
          },
        },
        recipients: true,
        alias: true,
      },
      orderBy: {
        createdAt: "desc",
      },
    });
  }
}
