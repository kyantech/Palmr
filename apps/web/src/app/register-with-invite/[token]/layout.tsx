import { Metadata } from "next";
import { getTranslations } from "next-intl/server";

interface LayoutProps {
  children: React.ReactNode;
}

export async function generateMetadata(): Promise<Metadata> {
  const t = await getTranslations();

  return {
    title: `${t("registerWithInvite.pageTitle")} `,
  };
}

export default function RegisterWithInviteLayout({ children }: LayoutProps) {
  return <>{children}</>;
}
