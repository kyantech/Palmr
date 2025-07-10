"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useTranslations } from "next-intl";

import { LoadingScreen } from "@/components/layout/loading-screen";
import { useAuth } from "@/contexts/auth-context";
import { getCurrentUser } from "@/http/endpoints";

export default function OIDCCallbackPage() {
  const router = useRouter();
  const { setUser, setIsAuthenticated, setIsAdmin } = useAuth();
  const t = useTranslations();

  useEffect(() => {
    const handleCallback = async () => {
      try {
        await new Promise((resolve) => setTimeout(resolve, 1000));

        const response = await getCurrentUser();
        const { isAdmin, ...userData } = response.data.user;

        setUser(userData);
        setIsAdmin(isAdmin);
        setIsAuthenticated(true);

        router.push("/dashboard");
      } catch (error) {
        console.error("OIDC callback error:", error);
        router.push("/login?error=authentication_failed");
      }
    };

    handleCallback();
  }, [router, setUser, setIsAuthenticated, setIsAdmin]);

  return (
    <div className="flex flex-col items-center justify-center min-h-screen">
      <LoadingScreen />
      <p className="mt-4 text-muted-foreground">{t("login.processing")}</p>
    </div>
  );
}
