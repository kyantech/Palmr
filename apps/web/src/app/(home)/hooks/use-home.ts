"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { create } from "zustand";

import { useAuth } from "@/contexts/auth-context";
import { useSecureConfigValue } from "@/hooks/use-secure-configs";

interface HomeStore {
  isLoading: boolean;
  shouldShowHomePage: boolean;
  setIsLoading: (loading: boolean) => void;
  setShouldShowHomePage: (show: boolean) => void;
}

const useHomeStore = create<HomeStore>((set) => ({
  isLoading: true,
  shouldShowHomePage: false,
  setIsLoading: (loading: boolean) => set({ isLoading: loading }),
  setShouldShowHomePage: (show: boolean) => set({ shouldShowHomePage: show }),
}));

export function useHome() {
  const router = useRouter();
  const { isLoading, shouldShowHomePage, setIsLoading, setShouldShowHomePage } = useHomeStore();
  const { isAuthenticated } = useAuth();
  const { value: showHomePage, isLoading: configLoading } = useSecureConfigValue("showHomePage");

  useEffect(() => {
    if (isAuthenticated === true) {
      router.replace("/dashboard");
      return;
    }
  }, [isAuthenticated, router]);

  useEffect(() => {
    if (!configLoading && isAuthenticated !== null) {
      setIsLoading(false);

      if (showHomePage !== "true") {
        router.push("/login");
        setShouldShowHomePage(false);
      } else if (isAuthenticated === false) {
        setShouldShowHomePage(true);
      }
    }
  }, [router, showHomePage, configLoading, isAuthenticated, setIsLoading, setShouldShowHomePage]);

  return {
    isLoading,
    shouldShowHomePage,
  };
}
