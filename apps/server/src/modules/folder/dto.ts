import { z } from "zod";

export const RegisterFolderSchema = z.object({
  name: z.string().min(1, "O nome da pasta é obrigatório"),
  description: z.string().optional(),
  objectName: z.string().min(1, "O objectName é obrigatório"),
  parentId: z.string().optional(),
});

export const UpdateFolderSchema = z.object({
  name: z.string().optional(),
  description: z.string().optional().nullable(),
});

export const MoveFolderSchema = z.object({
  parentId: z.string().nullable(),
});

export const FolderResponseSchema = z.object({
  id: z.string(),
  name: z.string(),
  description: z.string().nullable(),
  parentId: z.string().nullable(),
  userId: z.string(),
  createdAt: z.date(),
  updatedAt: z.date(),
  totalSize: z
    .bigint()
    .transform((val) => val.toString())
    .optional(),
  _count: z
    .object({
      files: z.number(),
      children: z.number(),
    })
    .optional(),
});

export const CheckFolderSchema = z.object({
  name: z.string().min(1, "O nome da pasta é obrigatório"),
  description: z.string().optional(),
  objectName: z.string().min(1, "O objectName é obrigatório"),
  parentId: z.string().optional(),
});

export const ListFoldersSchema = z.object({
  parentId: z.string().optional(),
  recursive: z.string().optional().default("true"),
});

export type RegisterFolderInput = z.infer<typeof RegisterFolderSchema>;
export type UpdateFolderInput = z.infer<typeof UpdateFolderSchema>;
export type MoveFolderInput = z.infer<typeof MoveFolderSchema>;
export type CheckFolderInput = z.infer<typeof CheckFolderSchema>;
export type ListFoldersInput = z.infer<typeof ListFoldersSchema>;
export type FolderResponse = z.infer<typeof FolderResponseSchema>;
