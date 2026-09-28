import { QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { ConfigProvider } from "antd";
import type { i18n as I18n } from "i18next";
import { Suspense } from "react";
import { I18nextProvider } from "react-i18next";
import { createMemoryRouter, type RouteObject } from "react-router";
import { RouterProvider } from "react-router/dom";
import { type BootState, BootStateContext } from "../app/bootstrap/bootState";
import type { LocaleCode } from "../app/i18n/catalog";
import { createI18n, preloadEagerNamespaces } from "../app/i18n/i18n";
import { createQueryClient } from "../shared/api/queryClient";

export async function loadedI18n(locale: LocaleCode = "en-US"): Promise<I18n> {
  const i18n = createI18n();
  await preloadEagerNamespaces(i18n, locale);
  await i18n.changeLanguage(locale);
  return i18n;
}

export interface RenderRouterOptions {
  state: BootState;
  initialEntries?: string[];
}

export async function renderRouter(
  routes: RouteObject[],
  { state, initialEntries = ["/"] }: RenderRouterOptions,
) {
  const i18n = await loadedI18n();
  const router = createMemoryRouter(routes, { initialEntries });
  const view = render(
    <QueryClientProvider client={createQueryClient()}>
      <I18nextProvider i18n={i18n}>
        <Suspense fallback={null}>
          <ConfigProvider>
            <BootStateContext value={state}>
              <RouterProvider router={router} />
            </BootStateContext>
          </ConfigProvider>
        </Suspense>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { router, view };
}
