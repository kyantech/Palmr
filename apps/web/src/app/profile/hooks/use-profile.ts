"use client";

import { useCallback, useEffect, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { z } from "zod";

import { useAuth } from "@/contexts/auth-context";
import { getCurrentUser, removeAvatar, updateUser, uploadAvatar } from "@/http/endpoints";

const createSchemas = (t: (key: string) => string) => ({
  profileSchema: z.object({
    firstName: z.string().min(1, t("validation.firstNameRequired")),
    lastName: z.string().min(1, t("validation.lastNameRequired")),
    username: z
      .string()
      .min(3, t("validation.usernameLength"))
      .regex(/^[^\s]+$/, t("validation.usernameSpaces")),
    email: z.string().email(t("validation.invalidEmail")),
  }),

  passwordSchema: z
    .object({
      newPassword: z.string().min(8, t("validation.passwordLength")),
      confirmPassword: z.string().min(8, t("validation.passwordLength")),
    })
    .refine((data) => data.newPassword === data.confirmPassword, {
      message: t("validation.passwordsMatch"),
      path: ["confirmPassword"],
    }),
});

export type PasswordFormData = z.infer<ReturnType<typeof createSchemas>["passwordSchema"]>;
export type ProfileFormData = z.infer<ReturnType<typeof createSchemas>["profileSchema"]>;

export function useProfile() {
  const t = useTranslations();
  const { profileSchema, passwordSchema } = createSchemas(t);

  const { setUser } = useAuth();
  const [isLoading, setIsLoading] = useState(true);
  const [userData, setUserData] = useState<any>(null);
  const [isNewPasswordVisible, setIsNewPasswordVisible] = useState(false);
  const [isConfirmPasswordVisible, setIsConfirmPasswordVisible] = useState(false);

  const profileForm = useForm<z.infer<typeof profileSchema>>({
    resolver: zodResolver(profileSchema),
  });

  const passwordForm = useForm<z.infer<typeof passwordSchema>>({
    resolver: zodResolver(passwordSchema),
  });

  const loadUserData = useCallback(async () => {
    try {
      const response = await getCurrentUser();

      setUserData(response.data.user);
      profileForm.reset({
        firstName: response.data.user.firstName,
        lastName: response.data.user.lastName,
        username: response.data.user.username,
        email: response.data.user.email,
      });
    } catch {
      toast.error(t("profile.errors.loadFailed"));
    } finally {
      setIsLoading(false);
    }
  }, [t, profileForm]);

  const onProfileSubmit = async (data: z.infer<typeof profileSchema>) => {
    const hasChanges = Object.keys(data).some((key) => data[key as keyof typeof data] !== userData[key]);

    if (!hasChanges) {
      toast.info(t("profile.messages.noChanges"));

      return;
    }

    try {
      await updateUser({
        id: userData.id,
        ...data,
      });
      toast.success(t("profile.messages.updateSuccess"));
      await loadUserData();
    } catch {
      toast.error(t("profile.errors.updateFailed"));
    }
  };

  const onPasswordSubmit = async (data: z.infer<typeof passwordSchema>) => {
    if (!data.newPassword || !data.confirmPassword) {
      toast.info(t("profile.messages.fillPasswords"));

      return;
    }

    try {
      await updateUser({
        id: userData.id,
        password: data.newPassword,
      });
      toast.success(t("profile.messages.passwordSuccess"));
      passwordForm.reset();
    } catch {
      toast.error(t("profile.errors.passwordFailed"));
    }
  };

  const handleImageChange = async (file: File) => {
    if (!file || !userData?.id) return;

    try {
      const response = await uploadAvatar({ file });
      const updatedUser = response.data;

      setUserData(updatedUser);
      setUser(updatedUser);
      toast.success(t("profile.messages.imageSuccess"));
    } catch {
      toast.error(t("profile.errors.imageFailed"));
    }
  };

  const handleImageRemove = async () => {
    if (!userData?.id) return;

    try {
      const response = await removeAvatar();
      const updatedUser = response.data;

      setUserData(updatedUser);
      setUser(updatedUser);
      toast.success(t("profile.messages.imageRemoved"));
    } catch {
      toast.error(t("profile.errors.imageRemoveFailed"));
    }
  };

  useEffect(() => {
    loadUserData();
  }, [loadUserData]);

  return {
    isLoading,
    userData,
    profileForm,
    passwordForm,
    isNewPasswordVisible,
    isConfirmPasswordVisible,
    setIsNewPasswordVisible,
    setIsConfirmPasswordVisible,
    onProfileSubmit,
    onPasswordSubmit,
    handleImageChange,
    handleImageRemove,
  };
}
