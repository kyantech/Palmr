import Link from "next/link";
import { useTranslations } from "next-intl";

import packageJson from "../../../../../../package.json";

const { version } = packageJson;

export function TransparentFooter() {
  const t = useTranslations();

  return (
    <footer className="absolute bottom-0 left-0 right-0 z-50 w-full flex items-center justify-center py-3 h-16 pointer-events-none">
      <div className="flex flex-col items-center pointer-events-auto">
        <Link
          target="_blank"
          className="flex items-center gap-1 text-white/80 hover:text-primary transition-colors"
          href="https://kyantech.com.br"
          title={t("footer.kyanHomepage")}
        >
          <span className="text-white/70 text-xs sm:text-sm">{t("footer.poweredBy")}</span>
          <p className="text-primary text-xs sm:text-sm font-medium cursor-pointer hover:text-primary/80">
            Kyantech Solutions
          </p>
        </Link>
        <span className="text-white text-[11px] mt-1">v{version}</span>
      </div>
    </footer>
  );
}
