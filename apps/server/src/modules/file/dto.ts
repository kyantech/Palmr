import { z } from "zod";

export const RegisterFileSchema = z.object({
  name: z.string().min(1, "O nome do arquivo é obrigatório"),
  description: z.string().optional(),
  extension: z.string().min(1, "A extensão é obrigatória"),
  size: z.number({
    required_error: "O tamanho é obrigatório",
    invalid_type_error: "O tamanho deve ser um número",
  }),
  objectName: z.string().min(1, "O objectName é obrigatório"),
  folderId: z.string().optional(),
});

export const CheckFileSchema = z.object({
  name: z.string().min(1, "O nome do arquivo é obrigatório"),
  description: z.string().optional(),
  extension: z.string().min(1, "A extensão é obrigatória"),
  size: z.number({
    required_error: "O tamanho é obrigatório",
    invalid_type_error: "O tamanho deve ser um número",
  }),
  objectName: z.string().min(1, "O objectName é obrigatório"),
  folderId: z.string().optional(),
});

export type RegisterFileInput = z.infer<typeof RegisterFileSchema>;
export type CheckFileInput = z.infer<typeof CheckFileSchema>;

export const UpdateFileSchema = z.object({
  name: z.string().optional().describe("The file name"),
  description: z.string().optional().nullable().describe("The file description"),
});

export const MoveFileSchema = z.object({
  folderId: z.string().nullable(),
});

export const ListFilesSchema = z.object({
  folderId: z.string().optional().describe("The folder ID"),
  recursive: z.string().optional().default("true").describe("Include files from subfolders"),
});

export type UpdateFileInput = z.infer<typeof UpdateFileSchema>;
export type MoveFileInput = z.infer<typeof MoveFileSchema>;
export type ListFilesInput = z.infer<typeof ListFilesSchema>;
