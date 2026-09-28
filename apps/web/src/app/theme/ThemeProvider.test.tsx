import { act, render, screen } from "@testing-library/react";
import { theme } from "antd";
import enUS from "antd/locale/en_US";
import { afterEach, describe, expect, expectTypeOf, test, vi } from "vitest";
import {
  ACCENT_PRESETS,
  type AccentKey,
  type Appearance,
  composeTheme,
  PALMR_PRIMARY_COLOR,
  safePrimaryColor,
  THEME_PREFERENCES,
  type ThemePreference,
  toAccentKey,
  toThemePreference,
} from "./appearance";
import { ThemeProvider } from "./ThemeProvider";
import { DARK_SCHEME_QUERY } from "./useResolvedThemeMode";

function controllableColorScheme(dark: boolean) {
  const listeners = new Set<() => void>();
  const media = {
    media: DARK_SCHEME_QUERY,
    matches: dark,
    addEventListener: vi.fn((_type: string, listener: () => void) => listeners.add(listener)),
    removeEventListener: vi.fn((_type: string, listener: () => void) => listeners.delete(listener)),
  };
  const matchMedia = vi.fn((query: string) => {
    expect(query).toBe(DARK_SCHEME_QUERY);
    return media;
  });
  vi.stubGlobal("matchMedia", matchMedia);
  return {
    listeners,
    media,
    setDark(next: boolean) {
      media.matches = next;
      act(() => {
        for (const listener of [...listeners]) {
          listener();
        }
      });
    },
  };
}

function TokenProbe() {
  const { token } = theme.useToken();
  return (
    <output data-testid="token">
      {JSON.stringify({ background: token.colorBgBase, primary: token.colorPrimary })}
    </output>
  );
}

function token(): { background: string; primary: string } {
  return JSON.parse(screen.getByTestId("token").textContent) as {
    background: string;
    primary: string;
  };
}

function themed(appearance: Partial<Appearance>) {
  return (
    <ThemeProvider
      appearance={{
        preference: "system",
        accent: "default",
        primaryColor: "#1668dc",
        ...appearance,
      }}
      direction="ltr"
      antdLocale={enUS}
      cspNonce={undefined}
    >
      <TokenProbe />
    </ThemeProvider>
  );
}

function documentTheme() {
  const root = document.documentElement;
  return { theme: root.dataset.theme, colorScheme: root.style.colorScheme };
}

const LIGHT_BACKGROUND = "#fff";
const DARK_BACKGROUND = "#000";

afterEach(() => {
  vi.unstubAllGlobals();
  delete document.documentElement.dataset.theme;
  document.documentElement.style.colorScheme = "";
});

test("component_theme_system_listener", () => {
  const scheme = controllableColorScheme(false);
  const view = render(themed({ preference: "system" }));

  expect(token().background).toBe(LIGHT_BACKGROUND);
  expect(documentTheme()).toEqual({ theme: "light", colorScheme: "light" });
  expect(scheme.listeners.size).toBe(1);

  scheme.setDark(true);
  expect(token().background).toBe(DARK_BACKGROUND);
  expect(documentTheme()).toEqual({ theme: "dark", colorScheme: "dark" });

  view.rerender(themed({ preference: "light" }));
  expect(scheme.listeners.size).toBe(0);
  expect(scheme.media.removeEventListener).toHaveBeenCalledTimes(1);
  expect(token().background).toBe(LIGHT_BACKGROUND);
  scheme.setDark(true);
  scheme.setDark(false);
  scheme.setDark(true);
  expect(token().background).toBe(LIGHT_BACKGROUND);
  expect(documentTheme().theme).toBe("light");

  view.rerender(themed({ preference: "dark" }));
  expect(scheme.listeners.size).toBe(0);
  scheme.setDark(false);
  expect(token().background).toBe(DARK_BACKGROUND);
  expect(documentTheme()).toEqual({ theme: "dark", colorScheme: "dark" });

  view.rerender(themed({ preference: "system" }));
  expect(scheme.listeners.size).toBe(1);
  expect(token().background).toBe(LIGHT_BACKGROUND);

  view.unmount();
  expect(scheme.listeners.size).toBe(0);
  expect(scheme.media.addEventListener).toHaveBeenCalledTimes(2);
  expect(scheme.media.removeEventListener).toHaveBeenCalledTimes(2);
});

test("without matchMedia, system resolves to light without subscribing", () => {
  vi.stubGlobal("matchMedia", undefined);

  render(themed({ preference: "system" }));

  expect(token().background).toBe(LIGHT_BACKGROUND);
});

describe("brand primary colour", () => {
  test("a valid #RRGGBB instance colour becomes colorPrimary", () => {
    controllableColorScheme(false);

    render(themed({ primaryColor: "#12ab34" }));

    expect(token().primary).toBe("#12ab34");
  });

  test.each([
    "red",
    "#abc",
    "#12ab345",
    "#12ab34; } body { display: none",
    "url(javascript:alert(1))",
    "",
    null,
    undefined,
  ])("%j falls back to the Palmr default", (value) => {
    expect(safePrimaryColor(value)).toBe(PALMR_PRIMARY_COLOR);
    expect(composeTheme("light", { accent: "default", primaryColor: value }).token).toEqual({
      colorPrimary: PALMR_PRIMARY_COLOR,
    });
  });
});

describe("accent presets", () => {
  test("default inherits the instance brand colour", () => {
    expect(composeTheme("light", { accent: "default", primaryColor: "#aa0000" }).token).toEqual({
      colorPrimary: "#aa0000",
    });
  });

  test("a curated accent overrides the instance brand colour in both modes", () => {
    for (const mode of ["light", "dark"] as const) {
      expect(composeTheme(mode, { accent: "rose", primaryColor: "#aa0000" }).token).toEqual({
        colorPrimary: ACCENT_PRESETS.rose,
      });
    }
    expect(composeTheme("dark", { accent: "rose", primaryColor: null }).algorithm).toBe(
      theme.darkAlgorithm,
    );
  });

  test("only the seven curated keys exist, each a fixed hex value", () => {
    expect(Object.keys(ACCENT_PRESETS)).toEqual([
      "default",
      "blue",
      "violet",
      "emerald",
      "amber",
      "rose",
      "slate",
    ]);
    const { default: inherited, ...curated } = ACCENT_PRESETS;
    expect(inherited).toBeNull();
    for (const value of Object.values(curated)) {
      expect(value).toMatch(/^#[0-9a-f]{6}$/);
    }
    expectTypeOf<AccentKey>().toEqualTypeOf<
      "default" | "blue" | "violet" | "emerald" | "amber" | "rose" | "slate"
    >();
    expectTypeOf<"#ff00ff">().not.toExtend<AccentKey>();
  });

  test("arbitrary server values cannot become an accent or a theme", () => {
    expect(toAccentKey("magenta")).toBe("default");
    expect(toAccentKey("#ff00ff")).toBe("default");
    expect(toAccentKey("__proto__")).toBe("default");
    expect(toAccentKey("toString")).toBe("default");
    expect(toAccentKey("emerald")).toBe("emerald");
    expect(toThemePreference("dark")).toBe("dark");
    expect(toThemePreference("sepia")).toBe("system");
    expect(THEME_PREFERENCES).toEqual(["light", "dark", "system"]);
    expectTypeOf<ThemePreference>().toEqualTypeOf<"light" | "dark" | "system">();
  });
});
