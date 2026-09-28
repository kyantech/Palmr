import { QueryClientProvider } from "@tanstack/react-query";
import { Typography } from "antd";
import { createQueryClient } from "../shared/api/queryClient";
import { RootErrorBoundary } from "./error/RootErrorBoundary";
import { createI18n } from "./i18n/i18n";
import { browserLanguages, resolveLocale } from "./i18n/resolveLocale";
import { PresentationProviders } from "./PresentationProviders";
import { DEFAULT_APPEARANCE } from "./theme/appearance";
import { readCspNonce } from "./theme/cspNonce";

const cspNonce = readCspNonce();
const queryClient = createQueryClient();
const i18n = createI18n();

export function App() {
  return (
    <RootErrorBoundary i18n={i18n}>
      <QueryClientProvider client={queryClient}>
        <PresentationProviders
          i18n={i18n}
          locale={resolveLocale({ browserLanguages: browserLanguages() })}
          appearance={DEFAULT_APPEARANCE}
          cspNonce={cspNonce}
        >
          <Typography.Title>Palmr</Typography.Title>
        </PresentationProviders>
      </QueryClientProvider>
    </RootErrorBoundary>
  );
}
