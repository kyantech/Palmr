"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { zodResolver } from "@hookform/resolvers/zod";
import axios from "axios";
import { useTranslations } from "next-intl";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { z } from "zod";

import { getAuthConfig, requestPasswordReset } from "@/http/endpoints";

export type ForgotPasswordFormData = {
  email: string;
};

export function useForgotPassword() {
  const t = useTranslations();
  const router = useRouter();
  const [passwordAuthEnabled, setPasswordAuthEnabled] = useState(true);
  const [authConfigLoading, setAuthConfigLoading] = useState(true);

  const forgotPasswordSchema = z.object({
    email: z.string().email(t("validation.invalidEmail")),
  });

  useEffect(() => {
    const fetchAuthConfig = async () => {
      try {
        const response = await getAuthConfig();
        setPasswordAuthEnabled((response as any).data.passwordAuthEnabled);
      } catch (error) {
        console.error("Failed to fetch auth config:", error);
        setPasswordAuthEnabled(true);
      } finally {
        setAuthConfigLoading(false);
      }
    };

    fetchAuthConfig();
  }, []);

  const form = useForm<ForgotPasswordFormData>({
    resolver: zodResolver(forgotPasswordSchema),
  });

  const onSubmit = async (data: ForgotPasswordFormData) => {
    if (!passwordAuthEnabled) {
      toast.error(t("errors.passwordAuthDisabled"));
      return;
    }

    try {
      await requestPasswordReset({
        email: data.email,
        origin: window.location.origin,
      });
      toast.success(t("forgotPassword.resetInstructions"));
      router.push("/login");
    } catch (err) {
      if (axios.isAxiosError(err) && err.response?.data?.message) {
        toast.error(t(err.response.data.message));
      } else {
        toast.error(t("common.unexpectedError"));
      }
    }
  };

  return {
    form,
    onSubmit,
    passwordAuthEnabled,
    authConfigLoading,
  };
}
