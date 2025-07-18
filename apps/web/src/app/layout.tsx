import { Outfit } from "next/font/google";
import { NextIntlClientProvider } from "next-intl";
import { getLocale } from "next-intl/server";

import "./globals.css";

import { GlobalDownloadToaster } from "@/components/downloads/global-download-toaster";
import { Favicon } from "@/components/layout/favicon";
import { DynamicToaster } from "@/components/ui/dynamic-toaster";
import { useAppInfo } from "@/contexts/app-info-context";
import { AuthProvider } from "@/contexts/auth-context";
import { ShareProvider } from "@/contexts/share-context";
import { DownloadManagerProvider } from "@/hooks/use-download-manager";
import { ThemeProvider } from "../providers/theme-provider";

const outfit = Outfit({
  subsets: ["latin"],
  variable: "--font-outfit",
  weight: ["100", "200", "300", "400", "500", "600", "700", "800", "900"],
  display: "swap",
});

export default async function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  const locale = await getLocale();
  const isRTL = locale === "ar-SA";

  if (typeof window !== "undefined") {
    useAppInfo.getState().refreshAppInfo();
  }

  return (
    <html lang={locale} dir={isRTL ? "rtl" : "ltr"} suppressHydrationWarning>
      <head>
        <Favicon />
      </head>
      <body className={`${outfit.variable} antialiased`}>
        <NextIntlClientProvider>
          <DownloadManagerProvider>
            <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
              <AuthProvider>
                <ShareProvider>{children}</ShareProvider>
              </AuthProvider>
              <DynamicToaster />
              <GlobalDownloadToaster />
            </ThemeProvider>
          </DownloadManagerProvider>
        </NextIntlClientProvider>
      </body>
    </html>
  );
}
