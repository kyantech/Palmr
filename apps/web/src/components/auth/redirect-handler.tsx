"use client";

import { useEffect } from "react";
import { usePathname, useRouter } from "next/navigation";

import { LoadingScreen } from "@/components/layout/loading-screen";
import { useAuth } from "@/contexts/auth-context";

interface RedirectHandlerProps {
  children: React.ReactNode;
}

const publicPaths = [
  "/login",
  "/forgot-password",
  "/reset-password",
  "/auth/callback",
  "/auth/oidc/callback",
  "/s/",
  "/r/",
];
const homePaths = ["/"];

export function RedirectHandler({ children }: RedirectHandlerProps) {
  const router = useRouter();
  const pathname = usePathname();
  const { isAuthenticated } = useAuth();

  useEffect(() => {
    if (isAuthenticated === true) {
      if (publicPaths.some((path) => pathname.startsWith(path)) || homePaths.includes(pathname)) {
        router.replace("/dashboard");
        return;
      }
    } else if (isAuthenticated === false) {
      if (!publicPaths.some((path) => pathname.startsWith(path)) && !homePaths.includes(pathname)) {
        router.replace("/login");
        return;
      }
    }
  }, [isAuthenticated, pathname, router]);

  if (isAuthenticated === null) {
    return <LoadingScreen />;
  }

  if (
    isAuthenticated === true &&
    (publicPaths.some((path) => pathname.startsWith(path)) || homePaths.includes(pathname))
  ) {
    return <LoadingScreen />;
  }

  if (
    isAuthenticated === false &&
    !publicPaths.some((path) => pathname.startsWith(path)) &&
    !homePaths.includes(pathname)
  ) {
    return <LoadingScreen />;
  }

  return <>{children}</>;
}
