"use client";

import { useRef, useState } from "react";
import { IconCamera, IconTrash } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { ImageEditModal } from "@/components/modals/image-edit-modal";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Card, CardHeader } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import type { ProfilePictureProps } from "../types";

export function ProfilePicture({ userData, onImageChange, onImageRemove }: ProfilePictureProps) {
  const t = useTranslations();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [isEditModalOpen, setIsEditModalOpen] = useState(false);
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  const handleAvatarClick = () => {
    fileInputRef.current?.click();
  };

  const handleFileChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];

    if (file) {
      setSelectedFile(file);
      setIsEditModalOpen(true);
    }
  };

  const handleImageEdit = async (croppedImageFile: File) => {
    setIsLoading(true);
    setIsEditModalOpen(false);
    setSelectedFile(null);
    // Reset the file input
    if (fileInputRef.current) {
      fileInputRef.current.value = "";
    }

    try {
      await onImageChange(croppedImageFile);
    } finally {
      setIsLoading(false);
    }
  };

  const handleEditModalClose = () => {
    setIsEditModalOpen(false);
    setSelectedFile(null);
    // Reset the file input
    if (fileInputRef.current) {
      fileInputRef.current.value = "";
    }
  };

  const handleImageRemove = async () => {
    setIsLoading(true);
    try {
      await onImageRemove();
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Card>
      <CardHeader className="flex flex-row items-center gap-4">
        <div className="relative group">
          {isLoading ? (
            <Skeleton className="w-25 h-25 rounded-full" />
          ) : (
            <Avatar className="w-25 h-25">
              <AvatarImage src={userData?.image} />
              <AvatarFallback className="absolute inset-0 rounded-full border text-4xl font-bold">
                {userData?.firstName
                  ? userData.firstName
                      .split(" ")
                      .map((firstName) => firstName[0])
                      .join("")
                      .toUpperCase()
                  : ""}
              </AvatarFallback>
            </Avatar>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                size="icon"
                className="absolute bottom-0 right-0 bg-primary text-primary-foreground rounded-full cursor-pointer"
                variant="default"
                disabled={isLoading}
              >
                <IconCamera className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              {!!userData?.image ? (
                <DropdownMenuItem className="text-destructive" onClick={handleImageRemove} disabled={isLoading}>
                  <IconTrash className="h-4 w-4" />
                  {t("profile.picture.removePhoto")}
                </DropdownMenuItem>
              ) : (
                <DropdownMenuItem onClick={handleAvatarClick} disabled={isLoading}>
                  <IconCamera className="h-4 w-4" />
                  {t("profile.picture.uploadPhoto")}
                </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
          <Input ref={fileInputRef} accept="image/*" className="hidden" type="file" onChange={handleFileChange} />
        </div>
        <div>
          <h2 className="text-lg font-semibold">{t("profile.picture.title")}</h2>
          <p className="text-sm text-muted-foreground">{t("profile.picture.description")}</p>
        </div>
      </CardHeader>

      <ImageEditModal
        isOpen={isEditModalOpen}
        onClose={handleEditModalClose}
        onSave={handleImageEdit}
        imageFile={selectedFile}
      />
    </Card>
  );
}
