import { afterEach, describe, expect, test } from "vitest";
import { resolveApiBase } from "../../shared/api/basePath";
import { resolveBasename } from "./basename";

afterEach(() => {
  document.head.querySelectorAll("base").forEach((element) => {
    element.remove();
  });
  window.history.replaceState(null, "", "/");
});

function setBase(href: string) {
  const base = document.createElement("base");
  base.href = href;
  document.head.append(base);
}

describe("unit_basename_from_base_uri", () => {
  test.each([
    ["https://example.com/", "/"],
    ["https://example.com/palmr/", "/palmr"],
    ["https://example.com/services/palmr/", "/services/palmr"],
    ["https://example.com/palmr", "/palmr"],
    ["https://example.com/palmr//", "/palmr"],
    ["https://example.com/palmr/?tab=files#top", "/palmr"],
    ["http://127.0.0.1:5487/", "/"],
  ])("%s → %s", (baseURI, basename) => {
    expect(resolveBasename(baseURI)).toBe(basename);
  });

  test.each([
    ["/", "/"],
    ["/palmr/", "/palmr"],
    ["/nested/palmr/", "/nested/palmr"],
  ])("<base href=%s> resolves to %s regardless of the current route depth", (href, basename) => {
    setBase(href);
    window.history.replaceState(null, "", `${href}files/019a/abc?sort=name#details`);

    expect(window.location.pathname).not.toBe(href);
    expect(resolveBasename()).toBe(basename);
  });

  test("the router basename and the API base come from the same primitive", () => {
    setBase("/nested/palmr/");
    window.history.replaceState(null, "", "/nested/palmr/settings/security");

    expect(resolveApiBase()).toBe(`${resolveBasename()}/api/v1`);
  });
});
