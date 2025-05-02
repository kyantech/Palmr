import { useEffect } from "react";
import { motion } from "framer-motion";
import { useTranslations } from "next-intl";

import { useAppInfo } from "@/contexts/app-info-context";

export function LoginHeader({ firstAccess }: { firstAccess: boolean }) {
  const t = useTranslations();
  const { appName, refreshAppInfo } = useAppInfo();

  useEffect(() => {
    refreshAppInfo();
  }, [refreshAppInfo]);

  return (
    <motion.div
      animate={{ opacity: 1 }}
      className="flex flex-col gap-1 flex-1 items-center justify-center text-center text-default-500 mb-4"
      initial={{ opacity: 0 }}
      transition={{ delay: 0.2, duration: 0.5 }}
    >
      <h1 className="text-2xl font-semibold tracking-tight text-center">
        {t("login.welcome")} {appName}
      </h1>
      {!firstAccess && <p className="text-default-500 text-sm">{t("login.signInToContinue")}</p>}
    </motion.div>
  );
}
