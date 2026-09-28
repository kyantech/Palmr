import type { i18n as I18n } from "i18next";
import { type ReactNode, Suspense } from "react";
import { I18nextProvider } from "react-i18next";

interface I18nProviderProps {
  i18n: I18n;
  children: ReactNode;
}

export function I18nProvider({ i18n, children }: I18nProviderProps) {
  return (
    <I18nextProvider i18n={i18n}>
      <Suspense fallback={null}>{children}</Suspense>
    </I18nextProvider>
  );
}
