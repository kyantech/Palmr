import { Banner } from "fumadocs-ui/components/banner";
import "./global.css";
import { RootProvider } from "fumadocs-ui/provider";
import { Inter } from "next/font/google";
import type { ReactNode } from "react";
import Link from "fumadocs-core/link";
import { LATEST_VERSION, LATEST_VERSION_PATH } from "@/config/constants";

const inter = Inter({
  subsets: ["latin"],
});

export const metadata = {
  title: "Palmr. | Official Website",
  description:
    "Palmr. is a fast, simple and powerful document sharing platform.",
};

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className={inter.className} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">
        <Banner variant="rainbow" id="banner-21-beta">
          <Link href={LATEST_VERSION_PATH}>
            Palmr. {LATEST_VERSION} has released!
          </Link>
        </Banner>
        <RootProvider
          search={{
            options: {
              defaultTag: "3.0-beta",
              tags: [
                {
                  name: "v1.1.7 Beta",
                  value: "1.1.7-beta",
                },
                {
                  name: "v2.0.0 Beta",
                  value: "2.0.0-beta",
                },
                {
                  name: "v3.0 Beta ✨",
                  value: "3.0-beta",
                  props: {
                    style: {
                      border: "1px solid rgba(0,165,80,0.2)",
                    },
                  },
                },
              ],
            },
          }}
        >
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
