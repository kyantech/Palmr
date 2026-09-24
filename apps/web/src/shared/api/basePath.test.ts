import { afterEach, expect, test } from "vitest";
import { resolveApiBase, resolveBasename } from "./basePath";

afterEach(() => {
  document.head.querySelectorAll("base").forEach((element) => {
    element.remove();
  });
});

test.each([
  ["https://files.example.com/", "/", "/api/v1"],
  ["https://files.example.com/palmr/", "/palmr", "/palmr/api/v1"],
  ["https://files.example.com/a/b//", "/a/b", "/a/b/api/v1"],
  ["http://127.0.0.1:5487/", "/", "/api/v1"],
])("%s resolves to basename %s and API base %s", (baseURI, basename, apiBase) => {
  expect(resolveBasename(baseURI)).toBe(basename);
  expect(resolveApiBase(baseURI)).toBe(apiBase);
});

test("the API base follows the document's <base href> and carries no origin", () => {
  const base = document.createElement("base");
  base.href = "/palmr/";
  document.head.append(base);

  expect(resolveBasename()).toBe("/palmr");
  expect(resolveApiBase()).toBe("/palmr/api/v1");
  expect(resolveApiBase()).not.toContain(window.location.host);
});
