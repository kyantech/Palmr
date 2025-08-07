"use client";

import { useEffect, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import axios from "axios";
import { useTranslations } from "next-intl";
import { toast } from "sonner";
import { z } from "zod";

import { useAuth } from "@/contexts/auth-context";
import { getAuthConfig, getCurrentUser, login } from "@/http/endpoints";
import { completeTwoFactorLogin } from "@/http/endpoints/auth/two-factor";
import type { LoginResponse } from "@/http/endpoints/auth/two-factor/types";
import { LoginFormValues } from "../schemas/schema";

export const loginSchema = z.object({
  emailOrUsername: z.string(),
  password: z.string(),
});

export type LoginFormData = z.infer<typeof loginSchema>;

export function useLogin() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const t = useTranslations();
  const { isAuthenticated, setUser, setIsAdmin, setIsAuthenticated } = useAuth();
  const [isVisible, setIsVisible] = useState(false);
  const [error, setError] = useState<string | undefined>();
  const [requiresTwoFactor, setRequiresTwoFactor] = useState(false);
  const [twoFactorUserId, setTwoFactorUserId] = useState<string | null>(null);
  const [twoFactorCode, setTwoFactorCode] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [passwordAuthEnabled, setPasswordAuthEnabled] = useState(true);
  const [authConfigLoading, setAuthConfigLoading] = useState(true);

  useEffect(() => {
    if (isAuthenticated === true) {
      router.replace("/dashboard");
    }
  }, [isAuthenticated, router]);

  useEffect(() => {
    const errorParam = searchParams.get("error");
    const messageParam = searchParams.get("message");

    if (errorParam) {
      let message: string;

      if (messageParam) {
        message = decodeURIComponent(messageParam);
      } else {
        const errorKey = `auth.errors.${errorParam}`;
        message = t(errorKey);
      }

      setTimeout(() => {
        toast.error(message);
      }, 100);

      setTimeout(() => {
        const url = new URL(window.location.href);
        url.searchParams.delete("error");
        url.searchParams.delete("message");
        url.searchParams.delete("provider");
        window.history.replaceState({}, "", url.toString());
      }, 1000);
    }
  }, [searchParams, t]);

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

  const toggleVisibility = () => setIsVisible(!isVisible);

  const onSubmit = async (data: LoginFormValues) => {
    setError(undefined);
    setIsSubmitting(true);

    try {
      if (!passwordAuthEnabled) {
        setError(t("errors.passwordAuthDisabled"));
        return;
      }

      const response = await login(data as any);
      const loginData = response.data as LoginResponse;

      if (loginData.requiresTwoFactor && loginData.userId) {
        setRequiresTwoFactor(true);
        setTwoFactorUserId(loginData.userId);
        return;
      }

      if (loginData.user) {
        try {
          const userResponse = await getCurrentUser();
          if (userResponse?.data?.user) {
            const { isAdmin, ...userData } = userResponse.data.user;
            setUser(userData);
            setIsAdmin(isAdmin);
            setIsAuthenticated(true);
            router.replace("/dashboard");
            return;
          }
        } catch (userErr) {
          console.warn("Failed to fetch complete user data, using login data:", userErr);
        }

        const { isAdmin, ...userData } = loginData.user;
        setUser({ ...userData, image: null });
        setIsAdmin(isAdmin);
        setIsAuthenticated(true);
        router.replace("/dashboard");
      }
    } catch (err) {
      if (axios.isAxiosError(err) && err.response?.data?.error) {
        setError(t(`errors.${err.response.data.error}`));
      } else {
        setError(t("errors.unexpectedError"));
      }
      setIsAuthenticated(false);
      setUser(null);
      setIsAdmin(false);
    } finally {
      setIsSubmitting(false);
    }
  };

  const onTwoFactorSubmit = async (rememberDevice: boolean = false) => {
    if (!twoFactorUserId || !twoFactorCode) {
      setError(t("twoFactor.messages.enterVerificationCode"));
      return;
    }

    setError(undefined);
    setIsSubmitting(true);

    try {
      const response = await completeTwoFactorLogin({
        userId: twoFactorUserId,
        token: twoFactorCode,
        rememberDevice: rememberDevice,
      });

      try {
        const userResponse = await getCurrentUser();
        if (userResponse?.data?.user) {
          const { isAdmin, ...userData } = userResponse.data.user;
          setUser(userData);
          setIsAdmin(isAdmin);
          setIsAuthenticated(true);
          router.replace("/dashboard");
          return;
        }
      } catch (userErr) {
        console.warn("Failed to fetch complete user data after 2FA, using response data:", userErr);
      }

      const { isAdmin, ...userData } = response.data.user;
      setUser({ ...userData, image: userData.image ?? null });
      setIsAdmin(isAdmin);
      setIsAuthenticated(true);
      router.replace("/dashboard");
    } catch (err) {
      if (axios.isAxiosError(err) && err.response?.data?.error) {
        setError(err.response.data.error);
      } else {
        setError(t("twoFactor.errors.invalidTwoFactorCode"));
      }
    } finally {
      setIsSubmitting(false);
    }
  };

  return {
    isAuthenticated,
    error,
    isVisible,
    toggleVisibility,
    onSubmit,
    requiresTwoFactor,
    twoFactorCode,
    setTwoFactorCode,
    onTwoFactorSubmit,
    isSubmitting,
    passwordAuthEnabled,
    authConfigLoading,
  };
}
