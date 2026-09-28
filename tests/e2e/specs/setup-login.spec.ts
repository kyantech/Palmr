import { expect, type Page, test } from "@playwright/test";

test.describe.configure({ mode: "serial" });

const APP_NAME = "Acme Files";
const ADMIN = {
  firstName: "Ada",
  lastName: "Lovelace",
  username: "ada",
  email: "ada@example.test",
  password: "correct horse battery staple",
};

function collectPageErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  return errors;
}

test("e2e_setup_login_overview_smoke", async ({ page, browser }) => {
  const pageErrors = collectPageErrors(page);
  const setupPosts: string[] = [];
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/v1/setup"
    ) {
      setupPosts.push(request.url());
    }
  });

  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Set up your instance" }),
  ).toBeVisible();
  const appName = page.getByLabel("Instance name");
  await expect(appName).toHaveValue("Palmr");
  await expect(page.getByText("Use at least 8 characters.")).toBeVisible();

  await appName.fill(APP_NAME);
  const language = page.getByLabel("Language");
  await language.click();
  await language.fill("Deutsch");
  await page.keyboard.press("Enter");
  await expect(
    page.getByText("Deutsch (Deutschland)").filter({ visible: true }).first(),
  ).toBeVisible();
  await page.getByLabel("First name").fill(ADMIN.firstName);
  await page.getByLabel("Last name").fill(ADMIN.lastName);
  await page.getByLabel("Username").fill(ADMIN.username);
  await page.getByLabel("E-mail").fill(ADMIN.email);
  await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
  await page.getByRole("button", { name: "Complete setup" }).dblclick();

  await expect(page).toHaveURL(/\/overview$/);
  expect(setupPosts).toHaveLength(1);
  const me = await page.request.get("/api/v1/auth/me");
  expect(me.status()).toBe(200);
  expect(await me.json()).toMatchObject({
    user: { username: ADMIN.username, role: "admin", locale: "de-DE" },
    restriction: null,
  });

  await page.goto("/setup");
  await expect(page).toHaveURL(/\/overview$/);

  const anonymous = await browser.newContext();
  const login = await anonymous.newPage();
  const loginErrors = collectPageErrors(login);
  await login.goto("/");
  await expect(login).toHaveURL(/\/login$/);
  await expect(
    login.getByRole("heading", { level: 1, name: "Sign in" }),
  ).toBeVisible();
  await expect(login.getByTestId("auth-brand")).toHaveText(APP_NAME);
  await expect(
    login.getByRole("link", { name: /register|sign up/i }),
  ).toHaveCount(0);

  await login.getByLabel("E-mail or username").fill("ADA@Example.test");
  await login.getByLabel("Password", { exact: true }).fill("not the password");
  await login.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(login.getByRole("alert")).toContainText(
    "Incorrect sign-in details",
  );
  await expect(login).toHaveURL(/\/login$/);

  await login.getByLabel("Password", { exact: true }).fill(ADMIN.password);
  await login.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(login).toHaveURL(/\/overview$/);
  expect((await login.request.get("/api/v1/auth/me")).status()).toBe(200);

  expect(pageErrors).toEqual([]);
  expect(loginErrors).toEqual([]);
  await anonymous.close();
});

test("it_setup_seeds_instance_identity", async ({ request }) => {
  const bootstrap = await request.get("/api/v1/bootstrap");
  expect(await bootstrap.json()).toMatchObject({
    setupCompleted: true,
    appName: APP_NAME,
    appDescription: "Self-hosted file transfer",
    defaultLocale: "de-DE",
  });
  const status = await request.get("/api/v1/setup/status");
  expect(await status.json()).toEqual({ setupCompleted: true });
});

test("regression_R069_setup_atomic_and_idempotent", async ({ page }) => {
  await page.goto("/setup");
  await expect(page).toHaveURL(/\/login$/);

  const attempts = await page.evaluate(async () => {
    const csrf =
      document.cookie
        .split(";")
        .map((pair) => pair.trim())
        .find((pair) => pair.startsWith("palmr_csrf="))
        ?.slice("palmr_csrf=".length) ?? "";
    const attempt = async (username: string) => {
      const response = await fetch("api/v1/setup", {
        method: "POST",
        credentials: "include",
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
          "X-Palmr-CSRF": csrf,
        },
        body: JSON.stringify({
          appName: "Hijacked",
          firstName: "Eve",
          lastName: "Mallory",
          username,
          email: `${username}@example.test`,
          password: "hijack password 1",
          locale: "en-US",
        }),
      });
      const body = (await response.json()) as { error?: { code?: string } };
      return { status: response.status, code: body.error?.code };
    };
    return Promise.all([attempt("eve1"), attempt("eve2")]);
  });

  expect(attempts).toEqual([
    { status: 409, code: "SETUP_ALREADY_COMPLETED" },
    { status: 409, code: "SETUP_ALREADY_COMPLETED" },
  ]);
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
  expect(
    await (await page.request.get("/api/v1/bootstrap")).json(),
  ).toMatchObject({
    appName: APP_NAME,
  });

  await page.getByLabel("E-mail or username").fill("eve1");
  await page.getByLabel("Password", { exact: true }).fill("hijack password 1");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Incorrect sign-in details",
  );
  await expect(page).toHaveURL(/\/login$/);
});
