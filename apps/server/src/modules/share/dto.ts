import { z } from "zod";

export const CreateShareSchema = z
  .object({
    name: z.string().optional().describe("The share name"),
    description: z.string().optional().describe("The share description"),
    expiration: z
      .string()
      .datetime({
        message: "Data de expiração deve estar no formato ISO 8601 (ex: 2025-02-06T13:20:49Z)",
      })
      .optional(),
    files: z.array(z.string()).optional().describe("The file IDs"),
    folders: z.array(z.string()).optional().describe("The folder IDs"),
    password: z.string().optional().describe("The share password"),
    maxViews: z.number().optional().nullable().describe("The maximum number of views"),
    recipients: z.array(z.string().email()).optional().describe("The recipient emails"),
  })
  .refine(
    (data) => {
      const hasFiles = data.files && data.files.length > 0;
      const hasFolders = data.folders && data.folders.length > 0;
      return hasFiles || hasFolders;
    },
    {
      message: "At least one file or folder must be selected to create a share",
    }
  );

export const UpdateShareSchema = z.object({
  id: z.string(),
  name: z.string().optional(),
  description: z.string().optional(),
  expiration: z.string().datetime().optional(),
  password: z.string().optional(),
  maxViews: z.number().optional().nullable(),
  recipients: z.array(z.string().email()).optional(),
});

export const ShareAliasResponseSchema = z.object({
  id: z.string(),
  alias: z.string(),
  shareId: z.string(),
  createdAt: z.string(),
  updatedAt: z.string(),
});

export const ShareResponseSchema = z.object({
  id: z.string().describe("The share ID"),
  name: z.string().nullable().describe("The share name"),
  description: z.string().nullable().describe("The share description"),
  expiration: z.string().nullable().describe("The share expiration date"),
  views: z.number().describe("The number of views"),
  createdAt: z.string().describe("The share creation date"),
  updatedAt: z.string().describe("The share update date"),
  creatorId: z.string().describe("The creator ID"),
  security: z.object({
    maxViews: z.number().nullable().describe("The maximum number of views"),
    hasPassword: z.boolean().describe("Whether the share has a password"),
  }),
  files: z.array(
    z.object({
      id: z.string().describe("The file ID"),
      name: z.string().describe("The file name"),
      description: z.string().nullable().describe("The file description"),
      extension: z.string().describe("The file extension"),
      size: z.string().describe("The file size"),
      objectName: z.string().describe("The file object name"),
      userId: z.string().describe("The user ID"),
      folderId: z.string().nullable().describe("The folder ID containing this file"),
      createdAt: z.string().describe("The file creation date"),
      updatedAt: z.string().describe("The file update date"),
    })
  ),
  folders: z.array(
    z.object({
      id: z.string().describe("The folder ID"),
      name: z.string().describe("The folder name"),
      description: z.string().nullable().describe("The folder description"),
      objectName: z.string().describe("The folder object name"),
      parentId: z.string().nullable().describe("The parent folder ID"),
      userId: z.string().describe("The user ID"),
      totalSize: z.string().nullable().describe("The total size of folder contents"),
      createdAt: z.string().describe("The folder creation date"),
      updatedAt: z.string().describe("The folder update date"),
      _count: z
        .object({
          files: z.number().describe("Number of files in folder"),
          children: z.number().describe("Number of subfolders"),
        })
        .optional(),
    })
  ),
  recipients: z.array(
    z.object({
      id: z.string().describe("The recipient ID"),
      email: z.string().email().describe("The recipient email"),
      createdAt: z.string().describe("The recipient creation date"),
      updatedAt: z.string().describe("The recipient update date"),
    })
  ),
  alias: ShareAliasResponseSchema.nullable(),
});

export const UpdateSharePasswordSchema = z.object({
  password: z.string().nullable().describe("The new password. Send null to remove password"),
});

export const UpdateShareItemsSchema = z
  .object({
    files: z.array(z.string().min(1, "File ID is required").describe("The file IDs")).optional(),
    folders: z.array(z.string().min(1, "Folder ID is required").describe("The folder IDs")).optional(),
  })
  .refine(
    (data) => {
      const hasFiles = data.files && data.files.length > 0;
      const hasFolders = data.folders && data.folders.length > 0;
      return hasFiles || hasFolders;
    },
    {
      message: "At least one file or folder must be provided",
    }
  );

export const UpdateShareRecipientsSchema = z.object({
  emails: z.array(z.string().email("Invalid email format").describe("The recipient emails")),
});

export const CreateShareAliasSchema = z.object({
  shareId: z.string().describe("The share ID"),
  alias: z
    .string()
    .regex(/^[a-zA-Z0-9]+$/, "Alias must contain only letters and numbers")
    .min(3, "Alias must be at least 3 characters long")
    .max(30, "Alias must not exceed 30 characters")
    .describe("The custom alias for the share"),
});

export const CreateShareWithFilesSchema = z.object({
  name: z.string().optional().describe("The share name"),
  description: z.string().optional().describe("The share description"),
  expiration: z
    .string()
    .datetime({
      message: "Data de expiração deve estar no formato ISO 8601 (ex: 2025-02-06T13:20:49Z)",
    })
    .optional(),
  existingFiles: z.array(z.string()).optional().describe("Existing file IDs to include"),
  existingFolders: z.array(z.string()).optional().describe("Existing folder IDs to include"),
  password: z.string().optional().describe("The share password"),
  maxViews: z.number().optional().nullable().describe("The maximum number of views"),
  recipients: z.array(z.string().email()).optional().describe("The recipient emails"),
  folderId: z.string().optional().nullable().describe("Folder ID to upload new files to"),
});

export type CreateShareInput = z.infer<typeof CreateShareSchema>;
export type CreateShareWithFilesInput = z.infer<typeof CreateShareWithFilesSchema>;
export type UpdateShareInput = z.infer<typeof UpdateShareSchema>;
export type ShareResponse = z.infer<typeof ShareResponseSchema>;
