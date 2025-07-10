"use client";

import { useEffect, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import axios from "axios";
import { useTranslations } from "next-intl";
import { toast } from "sonner";
import { z } from "zod";

import { useAuth } from "@/contexts/auth-context";
import { getAppInfo, getCurrentUser, login } from "@/http/endpoints";
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
  const [isInitialized, setIsInitialized] = useState(false);

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
    const checkAuth = async () => {
      try {
        const appInfoResponse = await getAppInfo();
        const appInfo = appInfoResponse.data;

        if (appInfo.firstUserAccess) {
          setUser(null);
          setIsAdmin(false);
          setIsAuthenticated(false);
          setIsInitialized(true);
          return;
        }

        const userResponse = await getCurrentUser();
        if (!userResponse?.data?.user) {
          throw new Error("No user data");
        }

        const { isAdmin, ...userData } = userResponse.data.user;
        setUser(userData);
        setIsAdmin(isAdmin);
        setIsAuthenticated(true);
        router.push("/dashboard");
      } catch (err) {
        console.error(err);
        setUser(null);
        setIsAdmin(false);
        setIsAuthenticated(false);
      } finally {
        setIsInitialized(true);
      }
    };

    checkAuth();
  }, [router, setUser, setIsAdmin, setIsAuthenticated]);

  const toggleVisibility = () => setIsVisible(!isVisible);

  const onSubmit = async (data: LoginFormValues) => {
    setError(undefined);

    try {
      await login(data);
      const userResponse = await getCurrentUser();
      const { isAdmin, ...userData } = userResponse.data.user;

      setUser(userData);
      setIsAdmin(isAdmin);
      setIsAuthenticated(true);
      router.replace("/dashboard");
    } catch (err) {
      if (axios.isAxiosError(err) && err.response?.data?.error) {
        setError(t(`errors.${err.response.data.error}`));
      } else {
        setError(t("errors.unexpectedError"));
      }
      setIsAuthenticated(false);
      setUser(null);
      setIsAdmin(false);
    }
  };

  return {
    isAuthenticated: !isInitialized ? null : isAuthenticated,
    error,
    isVisible,
    toggleVisibility,
    onSubmit,
  };
}
