import { render, screen } from "@testing-library/react";
import { Button, ConfigProvider } from "antd";
import enUS from "antd/locale/en_US";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { useContext } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { DEFAULT_APPEARANCE } from "./appearance";
import { readCspNonce } from "./cspNonce";
import { ThemeProvider } from "./ThemeProvider";

const UNSAFE_INLINE = ["unsafe", "inline"].join("-");

function CspProbe() {
  const { csp } = useContext(ConfigProvider.ConfigContext);
  return <output data-testid="csp">{JSON.stringify(csp ?? null)}</output>;
}

function sourceFiles(directory: string): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    return statSync(path).isDirectory() ? sourceFiles(path) : [path];
  });
}

afterEach(() => {
  document.head.querySelectorAll('meta[name="csp-nonce"]').forEach((meta) => {
    meta.remove();
  });
});

describe("reading the shell nonce", () => {
  test("the per-response meta nonce is read", () => {
    document.head.insertAdjacentHTML("beforeend", '<meta name="csp-nonce" content="r4nd0m">');

    expect(readCspNonce()).toBe("r4nd0m");
  });

  test("a missing or empty meta yields no nonce", () => {
    expect(readCspNonce()).toBeUndefined();
    document.head.insertAdjacentHTML("beforeend", '<meta name="csp-nonce" content="  ">');
    expect(readCspNonce()).toBeUndefined();
  });

  test("the nonce is never persisted in browser storage", () => {
    document.head.insertAdjacentHTML("beforeend", '<meta name="csp-nonce" content="r4nd0m">');

    readCspNonce();

    expect(window.sessionStorage).toHaveLength(0);
    expect(window.localStorage).toHaveLength(0);
  });
});

test("the shell nonce reaches the AntD style engine and every injected style", () => {
  render(
    <ThemeProvider
      appearance={DEFAULT_APPEARANCE}
      direction="ltr"
      antdLocale={enUS}
      cspNonce="sh3ll-n0nce"
    >
      <CspProbe />
      <Button>probe</Button>
    </ThemeProvider>,
  );

  expect(JSON.parse(screen.getByTestId("csp").textContent)).toEqual({ nonce: "sh3ll-n0nce" });
  const styles = [...document.head.querySelectorAll("style")];
  expect(styles.length).toBeGreaterThan(0);
  for (const style of styles) {
    expect(style.getAttribute("nonce")).toBe("sh3ll-n0nce");
  }
});

test("a missing nonce configures no CSP override and no unsafe inline-style fallback", () => {
  render(
    <ThemeProvider
      appearance={DEFAULT_APPEARANCE}
      direction="ltr"
      antdLocale={enUS}
      cspNonce={undefined}
    >
      <CspProbe />
    </ThemeProvider>,
  );

  expect(JSON.parse(screen.getByTestId("csp").textContent)).toBeNull();
  expect(document.documentElement.outerHTML).not.toContain(UNSAFE_INLINE);
  const offenders = sourceFiles(resolve(import.meta.dirname, "../..")).filter((path) =>
    readFileSync(path, "utf8").includes(UNSAFE_INLINE),
  );
  expect(offenders).toEqual([]);
});
