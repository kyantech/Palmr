"use client";

import { useEffect } from "react";
import { usePathname, useRouter } from "next/navigation";

import { publicPaths } from "@/components/auth/paths/public-paths";
import { unauthenticatedOnlyPaths } from "@/components/auth/paths/unahthenticated-only-paths";
import { LoadingScreen } from "@/components/layout/loading-screen";
import { useAuth } from "@/contexts/auth-context";

interface RedirectHandlerProps {
  children: React.ReactNode;
}

const homePaths = ["/"];

export function RedirectHandler({ children }: RedirectHandlerProps) {
  const router = useRouter();
  const pathname = usePathname();
  const { isAuthenticated } = useAuth();

  useEffect(() => {
    if (isAuthenticated === true) {
      if (unauthenticatedOnlyPaths.some((path) => pathname.startsWith(path)) || homePaths.includes(pathname)) {
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
    (unauthenticatedOnlyPaths.some((path) => pathname.startsWith(path)) || homePaths.includes(pathname))
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
