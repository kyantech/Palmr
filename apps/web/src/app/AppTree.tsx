import { type QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { i18n as I18n } from "i18next";
import { type ReactNode, useMemo } from "react";
import { RouterProvider, type RouterProviderProps } from "react-router/dom";
import { BootGate } from "./bootstrap/BootGate";
import { BootstrapProvider } from "./bootstrap/BootstrapProvider";
import { useBootState } from "./bootstrap/bootState";
import { bootPresentation } from "./bootstrap/presentation";
import { RootErrorBoundary } from "./error/RootErrorBoundary";
import type { StaleChunkRecovery } from "./error/staleChunk";
import type { LocaleLoaders } from "./i18n/useLocaleBridge";
import { PresentationProviders } from "./PresentationProviders";

export interface AppTreeProps {
  queryClient: QueryClient;
  i18n: I18n;
  router: RouterProviderProps["router"];
  cspNonce: string | undefined;
  browserLanguages: readonly string[];
  loaders?: LocaleLoaders;
  recovery?: StaleChunkRecovery;
}

interface BootedPresentationProps {
  i18n: I18n;
  cspNonce: string | undefined;
  browserLanguages: readonly string[];
  loaders: LocaleLoaders | undefined;
  children: ReactNode;
}

function BootedPresentation({
  i18n,
  cspNonce,
  browserLanguages,
  loaders,
  children,
}: BootedPresentationProps) {
  const state = useBootState();
  const { locale, appearance } = useMemo(
    () => bootPresentation(state, browserLanguages),
    [state, browserLanguages],
  );
  return (
    <PresentationProviders
      i18n={i18n}
      locale={locale}
      appearance={appearance}
      cspNonce={cspNonce}
      {...(loaders === undefined ? {} : { loaders })}
    >
      {children}
    </PresentationProviders>
  );
}

export function AppTree({
  queryClient,
  i18n,
  router,
  cspNonce,
  browserLanguages,
  loaders,
  recovery,
}: AppTreeProps) {
  return (
    <RootErrorBoundary i18n={i18n} {...(recovery === undefined ? {} : { recovery })}>
      <QueryClientProvider client={queryClient}>
        <BootstrapProvider>
          <BootGate>
            <BootedPresentation
              i18n={i18n}
              cspNonce={cspNonce}
              browserLanguages={browserLanguages}
              loaders={loaders}
            >
              <RouterProvider router={router} />
            </BootedPresentation>
          </BootGate>
        </BootstrapProvider>
      </QueryClientProvider>
    </RootErrorBoundary>
  );
}
