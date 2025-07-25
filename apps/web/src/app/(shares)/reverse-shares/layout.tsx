import { Metadata } from "next";
import { getTranslations } from "next-intl/server";

interface LayoutProps {
  children: React.ReactNode;
}

export async function generateMetadata(): Promise<Metadata> {
  const t = await getTranslations();

  return {
    title: `${t("shares.pageTitle")}`,
  };
}

export default function ReverseSharesLayout({ children }: LayoutProps) {
  return <>{children}</>;
}
