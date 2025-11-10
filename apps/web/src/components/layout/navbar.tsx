"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { IconLogout, IconPalette, IconSettings, IconUser, IconUsers } from "@tabler/icons-react";
import { useTranslations } from "next-intl";

import { LanguageSwitcher } from "@/components/general/language-switcher";
import { ModeToggle } from "@/components/general/mode-toggle";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAppInfo } from "@/contexts/app-info-context";
import { useAuth } from "@/contexts/auth-context";
import { logout as logoutAPI } from "@/http/endpoints";

export function Navbar() {
  const t = useTranslations();
  const router = useRouter();
  const { user, isAdmin, logout, isAuthenticated } = useAuth();
  const { appName, appLogo } = useAppInfo();
  const [isNavigating, setIsNavigating] = useState(false);

  const handleLogoClick = async () => {
    if (isNavigating || !isAuthenticated) return;

    try {
      setIsNavigating(true);
      router.replace("/dashboard");
    } catch (err) {
      console.error("Error navigating to dashboard:", err);
    } finally {
      setTimeout(() => setIsNavigating(false), 500);
    }
  };

  const handleLogout = async () => {
    try {
      await logoutAPI();
      logout();
      router.push("/login");
    } catch (err) {
      console.error("Error logging out:", err);
    }
  };

  return (
    <header className="sticky top-0 z-40 w-full border-b border-border/50 bg-background/70 backdrop-blur-sm px-6">
      <div className="container flex h-16 max-w-screen-xl items-center mx-auto lg:px-6">
        <div className="flex flex-1 items-center justify-between">
          <div className="flex items-center gap-3">
            <div
              onClick={handleLogoClick}
              className={`flex items-center gap-2 cursor-pointer transition-opacity ${
                isNavigating ? "opacity-50" : "opacity-100"
              }`}
            >
              {appLogo && <img alt={t("navbar.logoAlt")} className="h-8 w-8 object-contain rounded" src={appLogo} />}
              <p className="font-bold text-2xl">{appName}</p>
            </div>
          </div>

          <div className="flex items-center gap-2 cursor-pointer">
            <LanguageSwitcher />
            <ModeToggle />
            <DropdownMenu>
              <DropdownMenuTrigger className="rounded-full">
                <Avatar className="cursor-pointer h-10 w-10 rounded-full">
                  <AvatarImage src={user?.image as string | undefined} />
                  <AvatarFallback>{user?.firstName?.[0]}</AvatarFallback>
                </Avatar>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <div className="flex flex-col px-2 py-1.5 gap-0.5">
                  <p className="font-semibold text-sm">
                    {user?.firstName} {user?.lastName}
                  </p>
                  <p className="font-semibold text-xs text-muted-foreground">{user?.email}</p>
                </div>
                <DropdownMenuItem asChild>
                  <Link href="/profile" className="flex items-center gap-2 cursor-pointer">
                    <IconUser className="h-4 w-4" />
                    {t("navbar.profile")}
                  </Link>
                </DropdownMenuItem>
                <DropdownMenuItem asChild>
                  <Link href="/customization" className="flex items-center gap-2 cursor-pointer">
                    <IconPalette className="h-4 w-4" />
                    {t("navbar.customization")}
                  </Link>
                </DropdownMenuItem>
                {isAdmin && (
                  <>
                    <DropdownMenuItem asChild>
                      <Link href="/settings" className="flex items-center gap-2 cursor-pointer">
                        <IconSettings className="h-4 w-4" />
                        {t("navbar.settings")}
                      </Link>
                    </DropdownMenuItem>
                    <DropdownMenuItem asChild>
                      <Link href="/users-management" className="flex items-center gap-2 cursor-pointer">
                        <IconUsers className="h-4 w-4" />
                        {t("navbar.usersManagement")}
                      </Link>
                    </DropdownMenuItem>
                  </>
                )}
                <DropdownMenuItem
                  className="text-destructive focus:text-destructive cursor-pointer"
                  onClick={handleLogout}
                >
                  <IconLogout className="h-4 w-4 text-destructive" />
                  {t("navbar.logout")}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </div>
    </header>
  );
}
