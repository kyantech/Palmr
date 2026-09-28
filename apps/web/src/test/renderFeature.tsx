import { QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { ConfigProvider } from "antd";
import { type ReactNode, Suspense } from "react";
import { I18nextProvider } from "react-i18next";
import { createQueryClient } from "../shared/api/queryClient";
import { loadedI18n } from "./renderRouter";

export async function renderFeature(ui: ReactNode) {
  const i18n = await loadedI18n();
  const queryClient = createQueryClient();
  const view = render(
    <QueryClientProvider client={queryClient}>
      <I18nextProvider i18n={i18n}>
        <ConfigProvider theme={{ token: { motion: false } }}>
          <Suspense fallback={null}>{ui}</Suspense>
        </ConfigProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { queryClient, view };
}
