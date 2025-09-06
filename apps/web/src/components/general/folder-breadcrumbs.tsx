"use client";

import { IconHome } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";

interface Folder {
  id: string;
  name: string;
  description: string | null;
  parentId: string | null;
  userId: string;
  createdAt: string;
  updatedAt: string;
  totalSize?: string;
  _count?: {
    files: number;
    children: number;
  };
}

interface FolderBreadcrumbsProps {
  currentPath: Folder[];
  onNavigate: (folderId?: string) => void;
  onNavigateToRoot: () => void;
}

export function FolderBreadcrumbs({ currentPath, onNavigate, onNavigateToRoot }: FolderBreadcrumbsProps) {
  const t = useTranslations();

  return (
    <Breadcrumb>
      <BreadcrumbList>
        <BreadcrumbItem>
          <BreadcrumbLink className="flex items-center gap-1 cursor-pointer" onClick={onNavigateToRoot}>
            <IconHome size={16} />
            {t("folderActions.rootFolder")}
          </BreadcrumbLink>
        </BreadcrumbItem>

        {currentPath.map((folder, index) => (
          <div key={folder.id} className="contents">
            <BreadcrumbSeparator />
            <BreadcrumbItem>
              {index === currentPath.length - 1 ? (
                <BreadcrumbPage>{folder.name}</BreadcrumbPage>
              ) : (
                <BreadcrumbLink className="cursor-pointer" onClick={() => onNavigate(folder.id)}>
                  {folder.name}
                </BreadcrumbLink>
              )}
            </BreadcrumbItem>
          </div>
        ))}
      </BreadcrumbList>
    </Breadcrumb>
  );
}
