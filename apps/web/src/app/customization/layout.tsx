import { Metadata } from "next";
import { getTranslations } from "next-intl/server";

interface LayoutProps {
  children: React.ReactNode;
}

export async function generateMetadata(): Promise<Metadata> {
  const t = await getTranslations();

  return {
    title: `${t("customization.pageTitle")}`,
  };
}

export default function CustomizationLayout({ children }: LayoutProps) {
  return <>{children}</>;
}
