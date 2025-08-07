"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { IconHeart, IconMenu2 } from "@tabler/icons-react";

import { LanguageSwitcher } from "@/components/general/language-switcher";
import { ModeToggle } from "@/components/general/mode-toggle";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetTrigger } from "@/components/ui/sheet";
import { siteConfig } from "@/config/site";
import { useAppInfo } from "@/contexts/app-info-context";
import { cn } from "@/lib/utils";

export function Navbar() {
  const { appName, appLogo, refreshAppInfo } = useAppInfo();
  const [isMenuOpen, setIsMenuOpen] = useState(false);

  useEffect(() => {
    refreshAppInfo();
  }, [refreshAppInfo]);

  return (
    <header className="sticky top-0 z-40 w-full border-b border-border/50 bg-background/70 backdrop-blur-sm px-6">
      <div className="container flex h-16 max-w-screen-xl items-center mx-auto">
        <div className="flex flex-1 items-center justify-between">
          <div className="flex items-center gap-3">
            <Link href="/" className="flex items-center gap-2">
              {appLogo && <img alt="App Logo" className="h-8 w-8 object-contain rounded" src={appLogo} />}
              <p className="font-bold text-2xl">{appName}</p>
            </Link>
            <nav className="hidden md:flex ml-2 gap-4">
              {siteConfig.navItems.map((item) => (
                <Link
                  key={item.href}
                  href={item.href}
                  className={cn(
                    "text-sm font-medium transition-colors hover:text-foreground/80",
                    "data-[active=true]:text-primary data-[active=true]:font-medium"
                  )}
                >
                  {item.label}
                </Link>
              ))}
            </nav>
          </div>
          <div className="hidden lg:flex items-center gap-2">
            <LanguageSwitcher />
            <ModeToggle />

            <Button asChild variant="ghost" className="text-sm font-normal">
              <Link href={siteConfig.links.sponsor} target="_blank" rel="noopener noreferrer">
                <IconHeart className="h-4 w-4 text-destructive" />
                Sponsor
              </Link>
            </Button>
          </div>

          <div className="flex items-center gap-2 md:hidden">
            <LanguageSwitcher />
            <ModeToggle />
            <Sheet open={isMenuOpen} onOpenChange={setIsMenuOpen}>
              <SheetTrigger asChild>
                <Button variant="ghost" size="icon" className="h-9 w-9 p-0">
                  <IconMenu2 className="h-5 w-5" />
                  <span className="sr-only">Toggle menu</span>
                </Button>
              </SheetTrigger>
              <SheetContent side="right">
                <div className="grid gap-6 pt-6">
                  <div className="flex flex-col gap-4">
                    {siteConfig.navMenuItems.map((item, index) => (
                      <Link
                        key={`${item}-${index}`}
                        href={item.href}
                        className="text-foreground text-lg font-medium"
                        onClick={() => setIsMenuOpen(false)}
                      >
                        {item.label}
                      </Link>
                    ))}
                    <Link
                      href={siteConfig.links.sponsor}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-foreground text-lg font-medium flex items-center gap-2"
                      onClick={() => setIsMenuOpen(false)}
                    >
                      <IconHeart className="h-4 w-4 text-destructive" />
                      Sponsor
                    </Link>
                  </div>
                </div>
              </SheetContent>
            </Sheet>
          </div>
        </div>
      </div>
    </header>
  );
}
