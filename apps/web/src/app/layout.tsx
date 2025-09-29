import {
  Inter,
  Lato,
  Montserrat,
  Nunito,
  Open_Sans,
  Outfit,
  Poppins,
  Raleway,
  Roboto,
  Source_Sans_3,
  Work_Sans,
} from "next/font/google";
import { NextIntlClientProvider } from "next-intl";
import { getLocale } from "next-intl/server";

import "./globals.css";

import { RedirectHandler } from "@/components/auth/redirect-handler";
import { Favicon } from "@/components/layout/favicon";
import { DynamicToaster } from "@/components/ui/dynamic-toaster";
import { useAppInfo } from "@/contexts/app-info-context";
import { AuthProvider } from "@/contexts/auth-context";
import { ShareProvider } from "@/contexts/share-context";
import { ThemeColorProvider } from "../providers/theme-color-provider";
import { ThemeProvider } from "../providers/theme-provider";

const outfit = Outfit({
  subsets: ["latin"],
  variable: "--font-outfit",
  weight: ["100", "200", "300", "400", "500", "600", "700", "800", "900"],
  display: "swap",
});

const inter = Inter({
  subsets: ["latin"],
  variable: "--font-inter",
  display: "swap",
});

const roboto = Roboto({
  subsets: ["latin"],
  variable: "--font-roboto",
  weight: ["100", "300", "400", "500", "700", "900"],
  display: "swap",
});

const openSans = Open_Sans({
  subsets: ["latin"],
  variable: "--font-open-sans",
  display: "swap",
});

const poppins = Poppins({
  subsets: ["latin"],
  variable: "--font-poppins",
  weight: ["100", "200", "300", "400", "500", "600", "700", "800", "900"],
  display: "swap",
});

const nunito = Nunito({
  subsets: ["latin"],
  variable: "--font-nunito",
  display: "swap",
});

const lato = Lato({
  subsets: ["latin"],
  variable: "--font-lato",
  weight: ["100", "300", "400", "700", "900"],
  display: "swap",
});

const montserrat = Montserrat({
  subsets: ["latin"],
  variable: "--font-montserrat",
  display: "swap",
});

const sourceSans = Source_Sans_3({
  subsets: ["latin"],
  variable: "--font-source-sans",
  display: "swap",
});

const raleway = Raleway({
  subsets: ["latin"],
  variable: "--font-raleway",
  display: "swap",
});

const workSans = Work_Sans({
  subsets: ["latin"],
  variable: "--font-work-sans",
  display: "swap",
});

 const primaryLight = process.env.PUBLIC_PRIMARY_COLOR_LIGHT;
 const primaryDark = process.env.PUBLIC_PRIMARY_COLOR_DARK;

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
      <body
        className={`${outfit.variable} ${inter.variable} ${roboto.variable} ${openSans.variable} ${poppins.variable} ${nunito.variable} ${lato.variable} ${montserrat.variable} ${sourceSans.variable} ${raleway.variable} ${workSans.variable} font-sans antialiased`}
        style={{
          ...(primaryLight ? ({ ["--primary-light" as any]: primaryLight }) : null),
          ...(primaryDark ? ({ ["--primary-dark" as any]: primaryDark }) : null),
        }}
      >
        <NextIntlClientProvider>
          <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
            <ThemeColorProvider>
              <AuthProvider>
                <RedirectHandler>
                  <ShareProvider>{children}</ShareProvider>
                </RedirectHandler>
              </AuthProvider>
              <DynamicToaster />
            </ThemeColorProvider>
          </ThemeProvider>
        </NextIntlClientProvider>
      </body>
    </html>
  );
}
