import type { Metadata } from "next";
import { headers } from "next/headers";
import { getTranslations } from "next-intl/server";

async function getReverseShareMetadata(alias: string) {
  try {
    const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";
    const response = await fetch(`${API_BASE_URL}/reverse-shares/alias/${alias}/metadata`, {
      cache: "no-store",
    });

    if (!response.ok) {
      return null;
    }

    return await response.json();
  } catch (error) {
    console.error("Error fetching reverse share metadata:", error);
    return null;
  }
}

async function getAppInfo() {
  try {
    const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";
    const response = await fetch(`${API_BASE_URL}/app/info`, {
      cache: "no-store",
    });

    if (!response.ok) {
      return { appName: "Palmr", appDescription: "File sharing platform", appLogo: null };
    }

    return await response.json();
  } catch (error) {
    console.error("Error fetching app info:", error);
    return { appName: "Palmr", appDescription: "File sharing platform", appLogo: null };
  }
}

async function getBaseUrl(): Promise<string> {
  const headersList = await headers();
  // Handle multiple protocols in x-forwarded-proto (e.g., "https, https" from multiple proxies)
  const forwardedProto = headersList.get("x-forwarded-proto");
  const protocol = forwardedProto ? forwardedProto.split(",")[0].trim() : "http";
  const host = headersList.get("x-forwarded-host") || headersList.get("host") || "localhost:3000";
  return `${protocol}://${host}`;
}

export async function generateMetadata({ params }: { params: Promise<{ alias: string }> }): Promise<Metadata> {
  const t = await getTranslations();
  const resolvedParams = await params;
  const metadata = await getReverseShareMetadata(resolvedParams.alias);
  const appInfo = await getAppInfo();

  const title = metadata?.name || t("reverseShares.upload.metadata.title");
  const description =
    metadata?.description ||
    (metadata?.maxFiles
      ? t("reverseShares.upload.metadata.descriptionWithLimit", { limit: metadata.maxFiles })
      : t("reverseShares.upload.metadata.description"));

  const baseUrl = await getBaseUrl();
  const shareUrl = `${baseUrl}/r/${resolvedParams.alias}`;

  return {
    title,
    description,
    openGraph: {
      title,
      description,
      url: shareUrl,
      siteName: appInfo.appName || "Palmr",
      type: "website",
      images: appInfo.appLogo
        ? [
            {
              url: appInfo.appLogo,
              width: 1200,
              height: 630,
              alt: appInfo.appName || "Palmr",
            },
          ]
        : [],
    },
    twitter: {
      card: "summary_large_image",
      title,
      description,
      images: appInfo.appLogo ? [appInfo.appLogo] : [],
    },
  };
}

export default function ReverseShareLayout({ children }: { children: React.ReactNode }) {
  return children;
}
