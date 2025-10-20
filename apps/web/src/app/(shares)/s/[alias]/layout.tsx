import { Metadata } from "next";
import { headers } from "next/headers";
import { getTranslations } from "next-intl/server";

interface LayoutProps {
  children: React.ReactNode;
  params: { alias: string };
}

async function getShareMetadata(alias: string) {
  try {
    const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";
    const response = await fetch(`${API_BASE_URL}/shares/alias/${alias}/metadata`, {
      cache: "no-store",
    });

    if (!response.ok) {
      return null;
    }

    return await response.json();
  } catch (error) {
    console.error("Error fetching share metadata:", error);
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

function getBaseUrl(): string {
  const headersList = headers();
  const protocol = headersList.get("x-forwarded-proto") || "http";
  const host = headersList.get("x-forwarded-host") || headersList.get("host") || "localhost:3000";
  return `${protocol}://${host}`;
}

export async function generateMetadata({ params }: { params: { alias: string } }): Promise<Metadata> {
  const t = await getTranslations();
  const metadata = await getShareMetadata(params.alias);
  const appInfo = await getAppInfo();

  const title = metadata?.name || t("share.pageTitle");
  const description =
    metadata?.description ||
    (metadata?.totalFiles
      ? t("share.metadata.filesShared", { count: metadata.totalFiles + (metadata.totalFolders || 0) })
      : appInfo.appDescription || t("share.metadata.defaultDescription"));

  const baseUrl = getBaseUrl();
  const shareUrl = `${baseUrl}/s/${params.alias}`;

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

export default function DashboardLayout({ children }: LayoutProps) {
  return <>{children}</>;
}
