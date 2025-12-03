import { Banner } from "fumadocs-ui/components/banner";

import "./global.css";

import type { ReactNode } from "react";
import { Inter } from "next/font/google";
import Link from "fumadocs-core/link";
import { RootProvider } from "fumadocs-ui/provider";

import { LATEST_VERSION, LATEST_VERSION_PATH } from "@/config/constants";

const inter = Inter({
  subsets: ["latin"],
});

export const metadata = {
  title: "Palmr. | Official Website",
  description: "Palmr. is a fast, simple and powerful document sharing platform.",
};

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className={inter.className} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">
        <Banner variant="rainbow" id="banner-21-beta">
          <Link href={LATEST_VERSION_PATH}>Palmr. {LATEST_VERSION} has released!</Link>
        </Banner>
        <RootProvider>{children}</RootProvider>
      </body>
    </html>
  );
}
