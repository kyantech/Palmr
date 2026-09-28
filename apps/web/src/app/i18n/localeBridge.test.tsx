import { render, screen, waitFor } from "@testing-library/react";
import { ConfigProvider } from "antd";
import dayjs from "dayjs";
import { Component, type ReactElement, type ReactNode, useContext } from "react";
import { useTranslation } from "react-i18next";
import { afterEach, describe, expect, test, vi } from "vitest";
import { PresentationProviders } from "../PresentationProviders";
import { DEFAULT_APPEARANCE } from "../theme/appearance";
import { type LocaleCode, SUPPORTED_LOCALES } from "./catalog";
import { createI18n, loadNamespace } from "./i18n";
import { DAYJS_LOCALES, loadAntdLocale, loadDayjsLocale } from "./localeBridge";
import { DEFAULT_LOCALE_LOADERS, type LocaleLoaders } from "./useLocaleBridge";

function Probe({ namespace = "errors" }: { namespace?: string }) {
  const { direction, locale } = useContext(ConfigProvider.ConfigContext);
  const { t, i18n } = useTranslation(namespace);
  return (
    <output data-testid="probe">
      {JSON.stringify({
        direction,
        antd: locale?.locale,
        i18n: i18n.language,
        dayjs: dayjs.locale(),
        reload: t("boundary.reload", { ns: "errors" }),
      })}
    </output>
  );
}

class ErrorProbe extends Component<{ onError: (error: unknown) => void; children: ReactNode }> {
  override state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  override componentDidCatch(error: unknown) {
    this.props.onError(error);
  }

  override render() {
    return this.state.failed ? null : this.props.children;
  }
}

function probe(): Record<string, string> {
  return JSON.parse(screen.getByTestId("probe").textContent) as Record<string, string>;
}

function renderBridge(locale: LocaleCode, loaders: LocaleLoaders = DEFAULT_LOCALE_LOADERS) {
  const i18n = createI18n(loaders.namespace);
  const view = (code: LocaleCode, child = <Probe />) => (
    <PresentationProviders
      i18n={i18n}
      locale={code}
      appearance={DEFAULT_APPEARANCE}
      cspNonce={undefined}
      loaders={loaders}
    >
      {child}
    </PresentationProviders>
  );
  const result = render(view(locale));
  return {
    i18n,
    rerender: (code: LocaleCode, child?: ReactElement) => {
      result.rerender(view(code, child));
    },
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  dayjs.locale("en");
  document.documentElement.removeAttribute("lang");
  document.documentElement.removeAttribute("dir");
});

describe("one resolved locale drives every subsystem", () => {
  test("switching locale swaps the AntD, dayjs, i18next and document state together", async () => {
    const setDayjsLocale = vi.spyOn(dayjs, "locale");
    const { rerender } = renderBridge("en-US");

    await waitFor(() => {
      expect(probe()).toMatchObject({ i18n: "en-US", antd: "en", dayjs: "en", direction: "ltr" });
    });
    expect(probe().reload).toBe("Reload");
    expect(document.documentElement.lang).toBe("en-US");
    expect(document.documentElement.dir).toBe("ltr");

    rerender("ar-SA");
    await waitFor(() => {
      expect(probe()).toMatchObject({
        i18n: "ar-SA",
        antd: "ar",
        dayjs: "ar-sa",
        direction: "rtl",
      });
    });
    expect(probe().reload).toBe("إعادة التحميل");
    expect(document.documentElement.lang).toBe("ar-SA");
    expect(document.documentElement.dir).toBe("rtl");

    rerender("pt-BR");
    await waitFor(() => {
      expect(probe()).toMatchObject({
        i18n: "pt-BR",
        antd: "pt-br",
        dayjs: "pt-br",
        direction: "ltr",
      });
    });
    expect(probe().reload).toBe("Recarregar");
    expect(document.documentElement.dir).toBe("ltr");

    const switches = setDayjsLocale.mock.calls.filter(
      ([name, , isLocal]) => typeof name === "string" && !isLocal,
    );
    expect(switches.map(([name]) => name)).toEqual(["ar-sa", "pt-br"]);
  });

  test.each([
    ["fa-IR", "rtl"],
    ["he-IL", "rtl"],
    ["en-US", "ltr"],
    ["pt-BR", "ltr"],
  ] as const)("%s renders %s in the document and in AntD", async (locale, direction) => {
    renderBridge(locale);

    await waitFor(() => {
      expect(probe().direction).toBe(direction);
    });
    expect(document.documentElement.dir).toBe(direction);
    expect(document.documentElement.lang).toBe(locale);
  });
});

describe("namespaces", () => {
  test("errors and common load eagerly; surface namespaces load only when used", async () => {
    const namespace = vi.fn(loadNamespace);
    const { i18n, rerender } = renderBridge("de-DE", { ...DEFAULT_LOCALE_LOADERS, namespace });

    await waitFor(() => {
      expect(probe().i18n).toBe("de-DE");
    });
    expect(i18n.hasResourceBundle("de-DE", "errors")).toBe(true);
    expect(i18n.hasResourceBundle("en-US", "errors")).toBe(true);
    expect(i18n.hasResourceBundle("de-DE", "files")).toBe(false);
    const requested = () => namespace.mock.calls.map(([code, ns]) => `${code}/${ns}`).sort();
    expect(requested()).toEqual(["de-DE/common", "de-DE/errors", "en-US/common", "en-US/errors"]);

    rerender("de-DE", <Probe namespace="files" />);
    await waitFor(() => {
      expect(i18n.hasResourceBundle("de-DE", "files")).toBe(true);
    });
    expect(requested()).toContain("de-DE/files");
    expect(requested()).not.toContain("de-DE/admin");
  });

  test("the fallback chain is en-US only", () => {
    const i18n = createI18n();

    expect(i18n.options.fallbackLng).toEqual(["en-US"]);
    expect(i18n.options.load).toBe("currentOnly");
    expect(i18n.options.supportedLngs).not.toContain("en-XA");
  });

  test("an unknown namespace file is a load error, not an empty bundle", async () => {
    await expect(loadNamespace("en-XA", "errors")).rejects.toThrow(/No "errors" namespace/);
  });
});

test("every product locale has an AntD bundle and a dayjs locale", async () => {
  for (const locale of SUPPORTED_LOCALES) {
    const antd = await loadAntdLocale(locale);
    const dayjsName = await loadDayjsLocale(locale);

    expect(typeof antd.locale, locale).toBe("string");
    expect(dayjsName).toBe(DAYJS_LOCALES[locale].name);
    expect(dayjs.Ls[dayjsName], locale).toBeDefined();
  }
});

test("a failed locale bundle reaches the error boundary instead of rendering untranslated", async () => {
  const failure = new TypeError("Failed to fetch dynamically imported module: /assets/de_DE.js");
  const loaders = { ...DEFAULT_LOCALE_LOADERS, antd: () => Promise.reject(failure) };
  const caught = vi.fn();
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  const i18n = createI18n();

  render(
    <ErrorProbe onError={caught}>
      <PresentationProviders
        i18n={i18n}
        locale="de-DE"
        appearance={DEFAULT_APPEARANCE}
        cspNonce={undefined}
        loaders={loaders}
      >
        <Probe />
      </PresentationProviders>
    </ErrorProbe>,
  );

  await waitFor(() => {
    expect(caught).toHaveBeenCalledWith(failure);
  });
});
