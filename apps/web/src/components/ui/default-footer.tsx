"use client";

import Link from "next/link";
import { useTranslations } from "next-intl";

import { useSecureConfigValue } from "@/hooks/use-secure-configs";
import packageJson from "../../../package.json";

const { version } = packageJson;

export function DefaultFooter() {
  const t = useTranslations();
  const { value: hideVersion } = useSecureConfigValue("hideVersion");

  const shouldHideVersion = hideVersion === "true";

  return (
    <footer className="w-full flex items-center justify-center py-3 h-16">
      <div className="flex flex-col items-center">
        <Link
          target="_blank"
          className="flex items-center gap-1 text-current"
          href="https://kyantech.com.br"
          title={t("footer.kyanHomepage")}
        >
          <span className="text-default-600 text-xs sm:text-sm">{t("footer.poweredBy")}</span>
          <p className="text-primary text-xs sm:text-sm">Kyantech Solutions</p>
        </Link>
        {!shouldHideVersion && <span className="text-default-500 text-[11px] mt-1">v{version}</span>}
      </div>
    </footer>
  );
}
