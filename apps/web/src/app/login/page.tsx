"use client";

import { motion } from "framer-motion";

import { LanguageSwitcher } from "@/components/general/language-switcher";
import { LoadingScreen } from "@/components/layout/loading-screen";
import { DefaultFooter } from "@/components/ui/default-footer";
import { useAppInfo } from "@/contexts/app-info-context";
import { LoginForm } from "./components/login-form";
import { LoginHeader } from "./components/login-header";
import { RegisterForm } from "./components/register-form";
import { StaticBackgroundLights } from "./components/static-background-lights";
import { TwoFactorVerification } from "./components/two-factor-verification";
import { useLogin } from "./hooks/use-login";

export default function LoginPage() {
  const login = useLogin();
  const { firstAccess } = useAppInfo();

  if (login.isAuthenticated === null || login.isAuthenticated === true) {
    return <LoadingScreen />;
  }

  return (
    <div className="relative flex flex-col h-screen">
      <div className="fixed top-4 right-4 z-50">
        <LanguageSwitcher />
      </div>

      <div className="container mx-auto max-w-7xl px-6 flex-grow">
        <StaticBackgroundLights />
        <div className="relative flex h-full w-full items-center justify-center">
          <motion.div
            animate={{ opacity: 1, y: 0 }}
            className="flex w-full max-w-sm flex-col gap-4 rounded-lg bg-background/60 backdrop-blur-md px-8 pb-10 pt-6 shadow-lg border"
            initial={{ opacity: 0, y: 20 }}
            transition={{ duration: 0.5 }}
          >
            <LoginHeader firstAccess={firstAccess as boolean} />
            {firstAccess ? (
              <RegisterForm isVisible={login.isVisible} onToggleVisibility={login.toggleVisibility} />
            ) : login.requiresTwoFactor ? (
              <TwoFactorVerification
                twoFactorCode={login.twoFactorCode}
                setTwoFactorCode={login.setTwoFactorCode}
                onSubmit={login.onTwoFactorSubmit}
                error={login.error}
                isSubmitting={login.isSubmitting}
              />
            ) : (
              <LoginForm
                error={login.error}
                isVisible={login.isVisible}
                onSubmit={login.onSubmit}
                onToggleVisibility={login.toggleVisibility}
                passwordAuthEnabled={login.passwordAuthEnabled}
                authConfigLoading={login.authConfigLoading}
              />
            )}
          </motion.div>
        </div>
      </div>
      <DefaultFooter />
    </div>
  );
}
