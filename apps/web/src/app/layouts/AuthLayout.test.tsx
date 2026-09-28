import { screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { bootstrapFixture } from "../../test/bootFixtures";
import { renderRouter } from "../../test/renderRouter";
import { AUTH_BACKGROUND_POSITION, AUTH_BACKGROUND_SIZE, AuthLayout } from "./AuthLayout";

const PAGE_TITLE = "Sign in";

afterEach(() => {
  document.head.querySelectorAll("base").forEach((element) => {
    element.remove();
  });
});

async function renderLayout(bootstrap = bootstrapFixture(), backgroundUrl: string | null = null) {
  await renderRouter(
    [
      {
        element: <AuthLayout backgroundUrl={backgroundUrl} />,
        children: [{ path: "/login", element: <h1>{PAGE_TITLE}</h1> }],
      },
    ],
    { state: { bootstrap, me: null }, initialEntries: ["/login"] },
  );
  await screen.findByRole("heading", { level: 1 });
}

test("component_auth_layout_renders_instance_branding", async () => {
  const base = document.createElement("base");
  base.href = "/palmr/";
  document.head.append(base);

  await renderLayout(
    bootstrapFixture({ appName: "Acme Files", logoUrl: "/api/v1/public/branding/logo" }),
  );

  expect(screen.getByRole("main")).toBeDefined();
  const brand = screen.getByTestId("auth-brand");
  expect(brand.textContent).toBe("Acme Files");
  expect(brand.querySelector("img")?.getAttribute("src")).toBe(
    "/palmr/api/v1/public/branding/logo",
  );
  expect(brand.querySelector("img")?.getAttribute("alt")).toBe("");
});

test("a disabled logo renders the name only", async () => {
  await renderLayout(bootstrapFixture({ logoUrl: null }));

  expect(screen.getByTestId("auth-brand").querySelector("img")).toBeNull();
});

test("without a configured background the slot stays empty", async () => {
  await renderLayout(bootstrapFixture(), null);
  const plain = document.querySelector<HTMLElement>("[data-auth-background]");
  expect(plain?.dataset.authBackground).toBe("none");
  expect(plain?.style.backgroundImage ?? "").not.toContain("url(");
});

test("component_auth_layout_background_slot_uses_cover_center_center", async () => {
  await renderLayout(bootstrapFixture(), "/api/v1/public/branding/login-background");
  const layout = document.querySelector<HTMLElement>("[data-auth-background]");

  expect(AUTH_BACKGROUND_SIZE).toBe("cover");
  expect(AUTH_BACKGROUND_POSITION).toBe("center center");
  expect(layout?.dataset.authBackground).toBe("image");
  expect(layout?.style.backgroundImage).toContain("/api/v1/public/branding/login-background");
  expect(layout?.style.backgroundSize).toBe("cover");
  expect(layout?.style.backgroundPosition).toBe("center center");
});
