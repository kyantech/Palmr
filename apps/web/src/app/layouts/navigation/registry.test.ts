import { describe, expect, test } from "vitest";
import { PATHS } from "../../router/paths";
import {
  NAV_ENTRIES,
  type NavigationEntry,
  selectedNavigationKey,
  visibleNavigation,
} from "./registry";

interface EntryOverrides {
  key: string;
  order: number;
  path?: string;
  requiredRole?: NavigationEntry["requiredRole"];
}

function entry({ key, order, path, requiredRole }: EntryOverrides): NavigationEntry {
  return {
    key,
    order,
    path: path ?? `/${key}`,
    labelKey: "nav.overview",
    icon: null,
    ...(requiredRole === undefined ? {} : { requiredRole }),
  };
}

const FILES_ENTRY = entry({ key: "files", order: 20 });
const ADMIN_ENTRY = entry({ key: "admin", order: 60, requiredRole: "admin" });
const OVERVIEW_ENTRY = entry({ key: "overview", order: 10 });

describe("navigation registry", () => {
  test("lists only the concepts whose feature and route exist", () => {
    expect(NAV_ENTRIES.map((item) => item.path)).toEqual([PATHS.overview]);

    const registered = new Set(NAV_ENTRIES.map((item) => item.key));
    for (const concept of ["files", "shared", "received", "transfers", "admin", "settings"]) {
      expect(registered.has(concept)).toBe(false);
    }
  });

  test("every entry carries the metadata the desktop, drawer and bottom surfaces need", () => {
    for (const item of NAV_ENTRIES) {
      expect(item.key.length).toBeGreaterThan(0);
      expect(item.path.startsWith("/")).toBe(true);
      expect(item.labelKey.length).toBeGreaterThan(0);
      expect(item.icon).not.toBeNull();
      expect(Number.isFinite(item.order)).toBe(true);
    }
  });
});

describe("unit_navigation_role_visibility", () => {
  test("component_nav_admin_only_for_admin", () => {
    const forAdmin = visibleNavigation([FILES_ENTRY, ADMIN_ENTRY], "admin");
    const forUser = visibleNavigation([FILES_ENTRY, ADMIN_ENTRY], "user");

    expect(forAdmin.map((item) => item.key)).toEqual(["files", "admin"]);
    expect(forUser.map((item) => item.key)).toEqual(["files"]);
    expect(visibleNavigation([FILES_ENTRY], undefined).map((item) => item.key)).toEqual(["files"]);
  });

  test("entries are ordered by their declared order", () => {
    expect(
      visibleNavigation([ADMIN_ENTRY, FILES_ENTRY, OVERVIEW_ENTRY], "admin").map(
        (item) => item.key,
      ),
    ).toEqual(["overview", "files", "admin"]);
  });
});

describe("unit_navigation_selection", () => {
  const entries = [FILES_ENTRY, OVERVIEW_ENTRY];

  test("selects the entry matching the current path", () => {
    expect(selectedNavigationKey(entries, "/overview")).toBe("overview");
    expect(selectedNavigationKey(entries, "/files")).toBe("files");
  });

  test("keeps the parent entry selected for nested routes", () => {
    expect(selectedNavigationKey(entries, "/files/019a-abc")).toBe("files");
    expect(selectedNavigationKey(entries, "/files/search?q=report")).toBe("files");
  });

  test("prefers the longest matching prefix and otherwise selects nothing", () => {
    const nested = [
      entry({ key: "shares", path: "/shares", order: 30 }),
      entry({ key: "shared", path: "/shared", order: 40 }),
    ];
    expect(selectedNavigationKey(nested, "/shared/abc")).toBe("shared");
    expect(selectedNavigationKey(entries, "/elsewhere")).toBeNull();
    expect(selectedNavigationKey(entries, "/ov")).toBeNull();
  });
});
