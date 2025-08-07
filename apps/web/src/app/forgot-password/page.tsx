"use client";

import Link from "next/link";
import { motion } from "framer-motion";
import { useTranslations } from "next-intl";

import { DefaultFooter } from "@/components/ui/default-footer";
import { StaticBackgroundLights } from "../login/components/static-background-lights";
import { ForgotPasswordForm } from "./components/forgot-password-form";
import { ForgotPasswordHeader } from "./components/forgot-password-header";
import { useForgotPassword } from "./hooks/use-forgot-password";

export default function ForgotPasswordPage() {
  const forgotPassword = useForgotPassword();
  const t = useTranslations("ForgotPassword");

  return (
    <div className="relative flex min-h-screen flex-col">
      <div className="flex flex-1 items-center justify-center">
        <StaticBackgroundLights />
        <div className="relative z-10 w-full max-w-md space-y-4 px-4 py-12">
          <motion.div
            animate={{ opacity: 1, y: 0 }}
            className="rounded-2xl border border-default-200 bg-background/30 p-8"
            initial={{ opacity: 0, y: 20 }}
          >
            <ForgotPasswordHeader />
            {forgotPassword.authConfigLoading ? (
              <div className="flex justify-center items-center py-8">
                <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
              </div>
            ) : !forgotPassword.passwordAuthEnabled ? (
              <div className="mt-8 space-y-4">
                <div className="text-center p-4 bg-muted/50 rounded-lg">
                  <p className="text-muted-foreground">{t("forgotPassword.passwordAuthDisabled")}</p>
                </div>
                <div className="text-center">
                  <Link className="text-muted-foreground hover:text-primary text-sm" href="/login">
                    {t("forgotPassword.backToLogin")}
                  </Link>
                </div>
              </div>
            ) : (
              <ForgotPasswordForm form={forgotPassword.form} onSubmit={forgotPassword.onSubmit} />
            )}
          </motion.div>
        </div>
      </div>
      <DefaultFooter />
    </div>
  );
}
