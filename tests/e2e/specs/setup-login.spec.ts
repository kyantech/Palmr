import { type BrowserContext, expect, type Page, test } from "@playwright/test";

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

// The setup locale (de-DE) is persisted as the Admin's locale and the instance
// default, so the authenticated shell renders in German after both entry paths.
async function expectShellOverview(page: Page) {
  await expect(page).toHaveURL(/\/overview$/);
  await expect(page.getByTestId("app-shell")).toBeVisible();
  await expect(page.getByTestId("overview-page")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Übersicht");
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
  const german = page
    .getByTitle("Deutsch (Deutschland)", { exact: true })
    .filter({ visible: true });
  await german.click();
  await expect(language).toHaveAttribute("aria-expanded", "false");
  await expect(german).toHaveCount(1);
  await page.getByLabel("First name").fill(ADMIN.firstName);
  await page.getByLabel("Last name").fill(ADMIN.lastName);
  await page.getByLabel("Username").fill(ADMIN.username);
  await page.getByLabel("E-mail").fill(ADMIN.email);
  await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
  await page.getByRole("button", { name: "Complete setup" }).dblclick();

  await expectShellOverview(page);
  expect(setupPosts).toHaveLength(1);
  const me = await page.request.get("/api/v1/auth/me");
  expect(me.status()).toBe(200);
  expect(await me.json()).toMatchObject({
    user: { username: ADMIN.username, role: "admin", locale: "de-DE" },
    restriction: null,
  });

  await page.goto("/setup");
  await expectShellOverview(page);

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
  await expectShellOverview(login);
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

const NEW_PASSWORD = "an entirely different passphrase";

async function signIn(page: Page, identifier: string, password: string) {
  await page.goto("/login");
  await page.getByLabel("E-mail or username").fill(identifier);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.getByTestId("app-shell")).toBeVisible();
}

async function sessionStatus(page: Page): Promise<number> {
  return (await page.request.get("/api/v1/auth/me")).status();
}

async function currentSessionId(page: Page): Promise<string> {
  const me = (await (await page.request.get("/api/v1/auth/me")).json()) as {
    session: { id: string };
  };
  return me.session.id;
}

async function completeRecentAuthIfAsked(
  page: Page,
  outcome: ReturnType<Page["getByText"]>,
  password: string,
) {
  const dialog = page
    .getByRole("dialog")
    .filter({ hasText: "Confirm it's you" });
  await expect(outcome.or(dialog)).toBeVisible();
  if (await dialog.isVisible()) {
    await dialog.getByLabel("Password").fill(password);
    await dialog.getByRole("button", { name: "Confirm" }).click();
  }
  await expect(outcome).toBeVisible();
}

test("e2e_settings_appearance_profile_sessions_password", async ({
  browser,
}) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  const pageErrors = collectPageErrors(page);
  await signIn(page, ADMIN.username, ADMIN.password);

  await page.goto("/settings/appearance");
  const language = page.getByRole("combobox");
  await language.click();
  await language.fill("English");
  await page
    .getByTitle("English (United States)", { exact: true })
    .filter({ visible: true })
    .click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Settings");
  await page
    .getByRole("radiogroup", { name: "Theme" })
    .locator("label", { hasText: "Dark" })
    .click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(
    page.getByRole("status").filter({ hasText: "Saved" }),
  ).toBeVisible();
  await page
    .getByRole("radiogroup", { name: "Accent color" })
    .locator("label", { hasText: "Rose" })
    .click();
  await expect
    .poll(async () =>
      (await page.request.get("/api/v1/profile/preferences")).json(),
    )
    .toEqual({ locale: "en-US", theme: "dark", accent: "rose" });
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByRole("radio", { name: "Rose" })).toBeChecked();
  await expect(page.getByRole("radio", { name: "Dark" })).toBeChecked();

  await page.goto("/settings/profile");
  await expect(page.getByRole("textbox")).toHaveCount(2);
  await expect(page.getByLabel(/e-?mail|username|role/i)).toHaveCount(0);
  await page.getByLabel("First name").fill("Augusta");
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Profile saved.")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Account menu" }),
  ).toContainText("Augusta Lovelace");
  expect(
    await (await page.request.get("/api/v1/auth/me")).json(),
  ).toMatchObject({
    user: {
      firstName: "Augusta",
      lastName: ADMIN.lastName,
      username: ADMIN.username,
      email: ADMIN.email,
      role: "admin",
    },
  });

  const elsewhere = await browser.newContext();
  const elsewherePage = await elsewhere.newPage();
  await signIn(elsewherePage, ADMIN.email, ADMIN.password);
  const elsewhereId = await currentSessionId(elsewherePage);
  await page.goto("/settings/sessions");
  const elsewhereRow = page.locator(`[data-session-id="${elsewhereId}"]`);
  await expect(elsewhereRow).toBeVisible();
  const currentRow = page.locator('[data-current="true"]');
  await expect(currentRow).toHaveCount(1);
  await expect(currentRow).toHaveAttribute(
    "data-session-id",
    await currentSessionId(page),
  );
  await expect(currentRow.getByText("Current session")).toBeVisible();
  await elsewhereRow.getByRole("button", { name: /^Revoke session/ }).click();
  await page.getByRole("button", { name: "Revoke", exact: true }).click();
  await expect(elsewhereRow).toHaveCount(0);
  expect(await sessionStatus(elsewherePage)).toBe(401);
  expect(await sessionStatus(page)).toBe(200);

  const bystander = await browser.newContext();
  const bystanderPage = await bystander.newPage();
  await signIn(bystanderPage, ADMIN.username, ADMIN.password);
  await page.goto("/settings/security");
  await page.getByLabel("Current password").fill(ADMIN.password);
  await page.getByLabel("New password", { exact: true }).fill(NEW_PASSWORD);
  await page.getByLabel("Confirm new password").fill(NEW_PASSWORD);
  await page.getByRole("button", { name: "Change password" }).click();
  await completeRecentAuthIfAsked(
    page,
    page.getByText("Password changed."),
    ADMIN.password,
  );
  expect(await sessionStatus(page)).toBe(200);
  expect(await sessionStatus(bystanderPage)).toBe(401);

  const fresh = await browser.newContext();
  const freshPage = await fresh.newPage();
  await freshPage.goto("/login");
  await freshPage.getByLabel("E-mail or username").fill(ADMIN.username);
  await freshPage.getByLabel("Password", { exact: true }).fill(ADMIN.password);
  await freshPage.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(freshPage.getByRole("alert")).toContainText(
    "Incorrect sign-in details",
  );
  await signIn(freshPage, ADMIN.username, NEW_PASSWORD);

  await page.goto("/settings/sessions");
  await page.getByRole("button", { name: "Sign out other sessions" }).click();
  await page.getByRole("button", { name: "Sign out others" }).click();
  await completeRecentAuthIfAsked(
    page,
    page.getByText("All other sessions were signed out."),
    NEW_PASSWORD,
  );
  await expect(page.getByTestId("session-row")).toHaveCount(1);
  expect(await sessionStatus(freshPage)).toBe(401);
  expect(await sessionStatus(page)).toBe(200);

  await currentRow
    .getByRole("button", { name: /^Sign out of the current session/ })
    .click();
  await expect(
    page.getByText(
      "This ends your current session. You will need to sign in again.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page).toHaveURL(/\/login(\?|$)/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Sign in" }),
  ).toBeVisible();
  expect(await sessionStatus(page)).toBe(401);

  expect(pageErrors).toEqual([]);
  await Promise.all([
    context.close(),
    elsewhere.close(),
    bystander.close(),
    fresh.close(),
  ]);
});

const INVITEE = {
  firstName: "Grace",
  lastName: "Hopper",
  username: "grace",
  email: "grace@example.test",
  password: "an invited person's passphrase",
};
const INVITEE_NEW_PASSWORD = "a freshly chosen passphrase";

async function csrfOf(context: BrowserContext): Promise<string> {
  const cookies = await context.cookies();
  return cookies.find((cookie) => cookie.name === "palmr_csrf")?.value ?? "";
}

async function apiLogin(
  context: BrowserContext,
  identifier: string,
  password: string,
) {
  for (;;) {
    const response = await context.request.post("/api/v1/auth/login", {
      data: { identifier, password },
    });
    if (response.status() !== 429) {
      return response;
    }
    const wait = Number(response.headers()["retry-after"] ?? "1");
    await new Promise((resolve) => setTimeout(resolve, wait * 1000));
  }
}

test("regression_R068_user_self_service_profile_and_password", async ({
  browser,
}) => {
  const adminContext = await browser.newContext();
  const login = await apiLogin(adminContext, ADMIN.username, NEW_PASSWORD);
  expect(login.status()).toBe(200);
  const created = await adminContext.request.post("/api/v1/admin/invites", {
    headers: { "X-Palmr-CSRF": await csrfOf(adminContext) },
    data: { email: INVITEE.email, role: "user", sendEmail: false },
  });
  expect(created.status()).toBe(201);
  const { inviteUrl } = (await created.json()) as { inviteUrl: string };
  const token = new URL(inviteUrl).pathname.split("/").pop() ?? "";
  expect(new URL(inviteUrl).pathname).toBe(`/invite/${token}`);
  expect(token).toHaveLength(43);

  const userContext = await browser.newContext();
  const found = await userContext.request.get(
    `/api/v1/public/invites/${token}`,
  );
  expect(found.status()).toBe(200);
  expect(await found.json()).toMatchObject({
    valid: true,
    email: INVITEE.email,
  });
  const accepted = await userContext.request.post(
    `/api/v1/public/invites/${token}/accept`,
    {
      data: {
        firstName: INVITEE.firstName,
        lastName: INVITEE.lastName,
        username: INVITEE.username,
        password: INVITEE.password,
        locale: "en-US",
      },
    },
  );
  expect(accepted.status()).toBe(201);
  expect(await accepted.json()).toMatchObject({
    user: { username: INVITEE.username, role: "user" },
    mustChangePassword: false,
  });
  const reused = await userContext.request.post(
    `/api/v1/public/invites/${token}/accept`,
    {
      data: {
        firstName: "Eve",
        lastName: "Mallory",
        username: "eve",
        password: INVITEE.password,
        locale: "en-US",
      },
    },
  );
  expect(reused.status()).toBe(410);
  expect(
    ((await reused.json()) as { error: { code: string } }).error.code,
  ).toBe("INVITE_ALREADY_USED");

  const page = await userContext.newPage();
  const pageErrors = collectPageErrors(page);
  await page.goto("/settings/profile");
  await expect(page.getByTestId("app-shell")).toBeVisible();
  await expect(page.getByLabel(/e-?mail|username|role/i)).toHaveCount(0);
  await page.getByLabel("First name").fill("Grace M.");
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("Profile saved.")).toBeVisible();
  expect(
    await (await page.request.get("/api/v1/auth/me")).json(),
  ).toMatchObject({
    user: { firstName: "Grace M.", username: INVITEE.username, role: "user" },
  });

  const wrong = await page.request.post("/api/v1/profile/password", {
    headers: { "X-Palmr-CSRF": await csrfOf(userContext) },
    data: {
      currentPassword: "not the password",
      newPassword: INVITEE_NEW_PASSWORD,
    },
  });
  expect(wrong.status()).toBe(403);
  expect(((await wrong.json()) as { error: { code: string } }).error.code).toBe(
    "PASSWORD_CURRENT_INVALID",
  );

  await page.goto("/settings/security");
  await page.getByLabel("Current password").fill(INVITEE.password);
  await page
    .getByLabel("New password", { exact: true })
    .fill(INVITEE_NEW_PASSWORD);
  await page.getByLabel("Confirm new password").fill(INVITEE_NEW_PASSWORD);
  await page.getByRole("button", { name: "Change password" }).click();
  await completeRecentAuthIfAsked(
    page,
    page.getByText("Password changed."),
    INVITEE.password,
  );

  const denied = await page.request.get("/api/v1/admin/invites");
  expect(denied.status()).toBe(403);
  const fresh = await browser.newContext();
  const relogin = await apiLogin(fresh, INVITEE.username, INVITEE_NEW_PASSWORD);
  expect(relogin.status()).toBe(200);
  expect(await relogin.json()).toMatchObject({ user: { role: "user" } });

  expect(pageErrors).toEqual([]);
  await Promise.all([adminContext.close(), userContext.close(), fresh.close()]);
});
