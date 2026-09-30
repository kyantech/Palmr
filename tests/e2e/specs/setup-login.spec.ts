import {
  type Browser,
  type BrowserContext,
  expect,
  type Page,
  test,
} from "@playwright/test";
import { createHmac } from "node:crypto";
import {
  configureSmtpSink,
  operatorResetPassword,
  requireTwoFactor,
  runJobsOnce,
} from "../support/instance";
import { clearSink, messagesTo } from "../support/smtp-sink";

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

const HEDY = {
  firstName: "Hedy",
  lastName: "Lamarr",
  username: "hedy",
  email: "hedy@example.test",
  password: "frequency hopping passphrase",
};

const totp = { secret: "", lastStep: 0 };

function base32Decode(input: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = 0;
  let value = 0;
  const bytes: number[] = [];
  for (const character of input.replace(/\s|=/g, "").toUpperCase()) {
    value = (value << 5) | alphabet.indexOf(character);
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((value >>> bits) & 0xff);
    }
  }
  return Buffer.from(bytes);
}

function totpAt(secret: string, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const digest = createHmac("sha1", base32Decode(secret))
    .update(counter)
    .digest();
  const offset = (digest[digest.length - 1] ?? 0) & 0xf;
  return String(
    (digest.readUInt32BE(offset) & 0x7fffffff) % 1_000_000,
  ).padStart(6, "0");
}

// The server accepts one step of skew each side and refuses a step it has
// already seen, so each code comes from the earliest unused step in the window.
async function freshTotp(): Promise<string> {
  for (;;) {
    const now = Math.floor(Date.now() / 30_000);
    const step = [now - 1, now, now + 1].find(
      (candidate) => candidate > totp.lastStep,
    );
    if (step !== undefined) {
      totp.lastStep = step;
      return totpAt(totp.secret, step);
    }
    await new Promise((settle) => setTimeout(settle, 1_000));
  }
}

async function submitPasswordLogin(
  page: Page,
  identifier: string,
  password: string,
) {
  for (;;) {
    await page.goto("/login");
    await page.getByLabel("E-mail or username").fill(identifier);
    await page.getByLabel("Password", { exact: true }).fill(password);
    const [response] = await Promise.all([
      page.waitForResponse(
        (candidate) =>
          new URL(candidate.url()).pathname === "/api/v1/auth/login" &&
          candidate.request().method() === "POST",
      ),
      page.getByRole("button", { name: "Sign in", exact: true }).click(),
    ]);
    if (response.status() !== 429) {
      return response;
    }
    const wait = Number(response.headers()["retry-after"] ?? "1");
    await new Promise((settle) => setTimeout(settle, wait * 1000));
  }
}

async function signOutViaApi(context: BrowserContext) {
  const response = await context.request.post("/api/v1/auth/logout", {
    headers: { "X-Palmr-CSRF": await csrfOf(context) },
  });
  expect(response.status()).toBe(204);
}

async function confirmRecentAuthWithTotpIfAsked(
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
    const code = dialog.getByLabel("Authentication code");
    if (await code.isVisible()) {
      await code.fill(await freshTotp());
    }
    await dialog.getByRole("button", { name: "Confirm" }).click();
  }
  await expect(outcome).toBeVisible();
}

test("e2e_invite_accept", async ({ browser }) => {
  const adminContext = await browser.newContext();
  expect(
    (await apiLogin(adminContext, ADMIN.username, NEW_PASSWORD)).status(),
  ).toBe(200);
  const created = await adminContext.request.post("/api/v1/admin/invites", {
    headers: { "X-Palmr-CSRF": await csrfOf(adminContext) },
    data: { email: HEDY.email, role: "user", sendEmail: false },
  });
  expect(created.status()).toBe(201);
  const { inviteUrl } = (await created.json()) as { inviteUrl: string };
  const invitePath = new URL(inviteUrl).pathname;

  const context = await browser.newContext();
  const page = await context.newPage();
  const pageErrors = collectPageErrors(page);
  await page.goto(invitePath);
  await expect(
    page.getByRole("heading", { level: 1, name: "Create your account" }),
  ).toBeVisible();
  await expect(page.getByTestId("invite-email")).toHaveText(HEDY.email);
  await expect(page.locator("input[type='email']")).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: /e-?mail/i })).toHaveCount(0);

  const acceptBodies: unknown[] = [];
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/accept")) {
      acceptBodies.push(request.postDataJSON());
    }
  });
  await page.getByLabel("First name").fill(HEDY.firstName);
  await page.getByLabel("Last name").fill(HEDY.lastName);
  await page.getByLabel("Username").fill(HEDY.username);
  await page.getByLabel("Password", { exact: true }).fill(HEDY.password);
  await page.getByLabel("Confirm new password").fill(HEDY.password);
  await page.getByRole("button", { name: "Create account" }).click();

  await expect(page).toHaveURL(/\/overview$/);
  await expect(page.getByTestId("app-shell")).toBeVisible();
  expect(acceptBodies).toHaveLength(1);
  expect(Object.keys(acceptBodies[0] as object).sort()).toEqual(
    ["firstName", "lastName", "locale", "password", "username"].sort(),
  );
  expect(
    await (await page.request.get("/api/v1/auth/me")).json(),
  ).toMatchObject({
    user: { username: HEDY.username, email: HEDY.email, role: "user" },
    restriction: null,
  });

  const second = await browser.newContext();
  const secondPage = await second.newPage();
  await secondPage.goto(invitePath);
  await expect(
    secondPage.getByRole("heading", {
      level: 1,
      name: "This invite was already used",
    }),
  ).toBeVisible();
  await expect(secondPage).toHaveURL(new RegExp(`${invitePath}$`));
  const replay = await second.request.post(
    `/api/v1/public/invites/${invitePath.split("/").pop() ?? ""}/accept`,
    {
      data: {
        firstName: "Eve",
        lastName: "Mallory",
        username: "eve-again",
        password: HEDY.password,
        locale: "en-US",
      },
    },
  );
  expect(replay.status()).toBe(410);
  expect(
    ((await replay.json()) as { error: { code: string } }).error.code,
  ).toBe("INVITE_ALREADY_USED");

  expect(pageErrors).toEqual([]);
  await Promise.all([adminContext.close(), context.close(), second.close()]);
});

test("e2e_login_totp_trusted_device", async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  const pageErrors = collectPageErrors(page);

  expect(
    (await submitPasswordLogin(page, HEDY.username, HEDY.password)).status(),
  ).toBe(200);
  await expect(page.getByTestId("app-shell")).toBeVisible();
  await page.goto("/settings/security");
  await page
    .getByRole("button", { name: "Turn on two-factor authentication" })
    .click();
  await confirmRecentAuthWithTotpIfAsked(
    page,
    page.getByTestId("two-factor-secret"),
    HEDY.password,
  );
  totp.secret = (
    (await page.getByTestId("two-factor-secret").textContent()) ?? ""
  ).replace(/\s/g, "");
  expect(totp.secret).toMatch(/^[A-Z2-7]{16,}$/);
  await page.getByLabel("Authentication code").fill(await freshTotp());
  await page.getByRole("button", { name: "Verify and turn on" }).click();
  const codes = page.getByTestId("backup-code");
  await expect(codes).toHaveCount(10);
  for (const code of await codes.allTextContents()) {
    expect(code).toMatch(/^[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}$/);
  }
  await page
    .getByRole("checkbox", {
      name: "I saved these backup codes somewhere safe",
    })
    .check();
  await page.getByRole("button", { name: "Done" }).click();
  await expect(
    page.getByText("Two-factor authentication is on."),
  ).toBeVisible();

  await signOutViaApi(context);
  const challenged = await submitPasswordLogin(
    page,
    HEDY.username,
    HEDY.password,
  );
  expect(challenged.status()).toBe(401);
  await expect(page).toHaveURL(/\/login\/2fa$/);
  expect(
    (await context.cookies()).some((cookie) => cookie.name === "palmr_session"),
  ).toBe(false);
  await page.getByLabel("Authentication code").fill(await freshTotp());
  await page.getByRole("checkbox", { name: "Remember this device" }).check();
  await page.getByRole("button", { name: "Verify" }).click();
  await expect(page.getByTestId("app-shell")).toBeVisible();
  const device = (await context.cookies()).find(
    (cookie) => cookie.name === "palmr_device",
  );
  expect(device?.httpOnly).toBe(true);

  await signOutViaApi(context);
  expect(
    (await submitPasswordLogin(page, HEDY.username, HEDY.password)).status(),
  ).toBe(200);
  await expect(page.getByTestId("app-shell")).toBeVisible();
  await expect(page).not.toHaveURL(/\/login\/2fa/);

  await page.goto("/settings/security");
  const current = page.locator(
    '[data-testid="trusted-device-row"][data-current="true"]',
  );
  await expect(current).toHaveCount(1);
  await current.getByRole("button", { name: /^Remove trusted device/ }).click();
  await page.getByRole("button", { name: "Remove", exact: true }).click();
  await confirmRecentAuthWithTotpIfAsked(
    page,
    page.getByText("The trusted device was removed."),
    HEDY.password,
  );
  await expect(page.getByTestId("trusted-devices-empty")).toBeVisible();
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(200);

  await signOutViaApi(context);
  expect(
    (await submitPasswordLogin(page, HEDY.username, HEDY.password)).status(),
  ).toBe(401);
  await expect(page).toHaveURL(/\/login\/2fa$/);

  expect(pageErrors).toEqual([]);
  await context.close();
});

async function browserPersistence(page: Page) {
  return page.evaluate(async () => {
    const dump = (storage: Storage) =>
      Array.from({ length: storage.length }, (_, index) => {
        const key = storage.key(index) ?? "";
        return `${key}=${storage.getItem(key) ?? ""}`;
      }).join("\n");
    const records: string[] = [];
    for (const info of await indexedDB.databases()) {
      if (info.name === undefined) {
        continue;
      }
      const database = await new Promise<IDBDatabase>((done, fail) => {
        const request = indexedDB.open(info.name as string);
        request.onsuccess = () => done(request.result);
        request.onerror = () => fail(request.error);
      });
      for (const store of Array.from(database.objectStoreNames)) {
        const rows = await new Promise<unknown[]>((done, fail) => {
          const request = database
            .transaction(store)
            .objectStore(store)
            .getAll();
          request.onsuccess = () => done(request.result);
          request.onerror = () => fail(request.error);
        });
        records.push(`${info.name}/${store}:${JSON.stringify(rows)}`);
      }
      database.close();
    }
    return {
      cookie: document.cookie,
      local: dump(localStorage),
      session: dump(sessionStorage),
      indexedDb: records.join("\n"),
      url: location.href,
      history: JSON.stringify(history.state),
    };
  });
}

test("it_mfa_token_browser_contract", async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  const response = await submitPasswordLogin(
    page,
    HEDY.username,
    HEDY.password,
  );
  expect(response.status()).toBe(401);
  const body = (await response.json()) as {
    error: { code: string; details: { mfaToken?: string } };
  };
  expect(body.error.code).toBe("AUTH_2FA_REQUIRED");
  const token = body.error.details.mfaToken ?? "";
  expect(token.length).toBeGreaterThan(16);
  await expect(page).toHaveURL(/\/login\/2fa$/);
  await expect(page.getByLabel("Authentication code")).toBeVisible();

  const assertAbsent = async () => {
    const stored = await browserPersistence(page);
    for (const value of Object.values(stored)) {
      expect(value).not.toContain(token);
    }
    for (const cookie of await context.cookies()) {
      expect(cookie.value).not.toContain(token);
      expect(cookie.name).not.toContain(token);
    }
    expect(page.url()).not.toContain(token);
    expect(await page.content()).not.toContain(token);
  };

  await assertAbsent();
  await page.reload();
  await expect(page).toHaveURL(/\/login$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Sign in" }),
  ).toBeVisible();
  await assertAbsent();
  await context.close();
});

const APP_ORIGIN = process.env.PALMR_E2E_BASE_URL ?? "http://127.0.0.1:5487";

const KATHERINE = {
  firstName: "Katherine",
  lastName: "Johnson",
  username: "katherine",
  email: "katherine@example.test",
  password: "orbital mechanics passphrase",
};
const KATHERINE_NEW_PASSWORD = "a passphrase chosen from the e-mailed link";

async function createAccountThroughInvite(
  browser: Browser,
  account: typeof KATHERINE,
) {
  const adminContext = await browser.newContext();
  expect(
    (await apiLogin(adminContext, ADMIN.username, NEW_PASSWORD)).status(),
  ).toBe(200);
  const created = await adminContext.request.post("/api/v1/admin/invites", {
    headers: { "X-Palmr-CSRF": await csrfOf(adminContext) },
    data: { email: account.email, role: "user", sendEmail: false },
  });
  expect(created.status()).toBe(201);
  const { inviteUrl } = (await created.json()) as { inviteUrl: string };
  const token = new URL(inviteUrl).pathname.split("/").pop() ?? "";
  const accountContext = await browser.newContext();
  const accepted = await accountContext.request.post(
    `/api/v1/public/invites/${token}/accept`,
    {
      data: {
        firstName: account.firstName,
        lastName: account.lastName,
        username: account.username,
        password: account.password,
        locale: "en-US",
      },
    },
  );
  expect(accepted.status()).toBe(201);
  await Promise.all([adminContext.close(), accountContext.close()]);
}

test("e2e_password_reset_via_smtp_sink", async ({ browser, baseURL }) => {
  test.setTimeout(120_000);
  const origin = baseURL ?? APP_ORIGIN;
  await createAccountThroughInvite(browser, KATHERINE);
  await configureSmtpSink(origin);
  await clearSink();

  const context = await browser.newContext();
  const page = await context.newPage();
  const pageErrors = collectPageErrors(page);
  await page.goto("/login");
  await page.getByRole("button", { name: "Forgot password?" }).click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Reset your password" }),
  ).toBeVisible();
  const forgotBodies: unknown[] = [];
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/v1/auth/password/forgot"
    ) {
      forgotBodies.push(request.postDataJSON());
    }
  });
  await page.getByLabel("E-mail or username").fill(KATHERINE.email);
  await page.getByRole("button", { name: "Send reset instructions" }).click();
  await expect(page.getByTestId("forgot-password-sent")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 1, name: "Request received" }),
  ).toBeVisible();
  await expect(page.locator("body")).not.toContainText(KATHERINE.email);
  expect(forgotBodies).toEqual([{ identifier: KATHERINE.email }]);

  await runJobsOnce(origin, "email.send");
  await expect
    .poll(async () => (await messagesTo(KATHERINE.email)).length)
    .toBe(1);
  const [message] = await messagesTo(KATHERINE.email);
  const link = /https?:\/\/[^\s"'<>]+\/reset-password\/[A-Za-z0-9_-]+/.exec(
    message?.Text ?? "",
  )?.[0];
  expect(link).toBeDefined();
  const resetUrl = new URL(link ?? "");
  expect(resetUrl.origin).toBe(new URL(origin).origin);
  expect(resetUrl.pathname).toMatch(/^\/reset-password\/[A-Za-z0-9_-]{43}$/);
  expect(resetUrl.search).toBe("");

  const checks: number[] = [];
  const resets: unknown[] = [];
  page.on("response", (response) => {
    const path = new URL(response.url()).pathname;
    if (path === "/api/v1/auth/password/reset/check") {
      checks.push(response.status());
    }
  });
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/v1/auth/password/reset"
    ) {
      resets.push(request.postDataJSON());
    }
  });
  await page.goto(resetUrl.pathname);
  await expect(
    page.getByRole("heading", { level: 1, name: "Choose a new password" }),
  ).toBeVisible();
  expect(checks).toEqual([200]);
  await expect(page.locator("body")).not.toContainText(KATHERINE.email);
  await page
    .getByLabel("New password", { exact: true })
    .fill(KATHERINE_NEW_PASSWORD);
  await page.getByLabel("Confirm new password").fill(KATHERINE_NEW_PASSWORD);
  await page.getByRole("button", { name: "Reset password" }).click();

  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByText("Your password was reset")).toBeVisible();
  expect(resets).toEqual([
    {
      token: resetUrl.pathname.split("/").pop(),
      newPassword: KATHERINE_NEW_PASSWORD,
    },
  ]);
  expect(
    (await context.cookies()).some((cookie) => cookie.name === "palmr_session"),
  ).toBe(false);

  const fresh = await browser.newContext();
  const old = await apiLogin(fresh, KATHERINE.username, KATHERINE.password);
  expect(old.status()).toBe(401);
  expect(((await old.json()) as { error: { code: string } }).error.code).toBe(
    "AUTH_INVALID_CREDENTIALS",
  );
  expect(
    (
      await apiLogin(fresh, KATHERINE.username, KATHERINE_NEW_PASSWORD)
    ).status(),
  ).toBe(200);

  expect(pageErrors).toEqual([]);
  await Promise.all([context.close(), fresh.close()]);
});

test("e2e_forced_password_change", async ({ browser, baseURL }) => {
  test.setTimeout(120_000);
  const probe = await browser.newContext();
  expect(
    (await apiLogin(probe, INVITEE.username, INVITEE_NEW_PASSWORD)).status(),
  ).toBe(200);
  const { user } = (await (
    await probe.request.get("/api/v1/auth/me")
  ).json()) as {
    user: { id: string };
  };
  await probe.close();

  const origin = baseURL ?? APP_ORIGIN;
  const temporary = await operatorResetPassword(origin, user.id);

  const context = await browser.newContext();
  const page = await context.newPage();
  const pageErrors = collectPageErrors(page);
  expect(
    (await submitPasswordLogin(page, INVITEE.username, temporary)).status(),
  ).toBe(200);
  await expect(page).toHaveURL(/\/login\/forced-password-change/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Set a new password" }),
  ).toBeVisible();
  await expect(
    page.getByText("You signed in with a temporary password"),
  ).toBeVisible();
  await expect(page.getByTestId("app-shell")).toHaveCount(0);
  await expect(page.getByTestId("auth-brand")).toBeVisible();
  await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible();
  await expect(page.getByLabel("Current password")).toHaveCount(0);

  await page.goto("/settings/profile");
  await expect(page).toHaveURL(/\/login\/forced-password-change/);
  await expect(page.getByTestId("app-shell")).toHaveCount(0);
  const blocked = await page.request.get("/api/v1/profile");
  expect(blocked.status()).toBe(403);
  expect(
    ((await blocked.json()) as { error: { code: string } }).error.code,
  ).toBe("AUTH_PASSWORD_CHANGE_REQUIRED");

  await page.getByLabel("New password", { exact: true }).fill("short");
  await page.getByLabel("Confirm new password").fill("short");
  await page.getByRole("button", { name: "Set new password" }).click();
  await expect(
    page.getByText("Use at least 8 characters.").first(),
  ).toBeVisible();

  const oldSession = (await context.cookies()).filter(
    (cookie) => cookie.name === "palmr_session",
  );
  expect(oldSession).toHaveLength(1);
  const passwordBodies: unknown[] = [];
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/v1/profile/password"
    ) {
      passwordBodies.push(request.postDataJSON());
    }
  });
  const chosen = "a password chosen after the reset";
  await page.getByLabel("New password", { exact: true }).fill(chosen);
  await page.getByLabel("Confirm new password").fill(chosen);
  await page.getByRole("button", { name: "Set new password" }).click();

  await expect(page).toHaveURL(/\/settings\/profile$/);
  await expect(page.getByTestId("app-shell")).toBeVisible();
  expect(passwordBodies).toEqual([{ newPassword: chosen }]);
  const rotated = (await context.cookies()).find(
    (cookie) => cookie.name === "palmr_session",
  );
  expect(rotated?.value).not.toBe(oldSession[0]?.value);
  expect(
    await (await page.request.get("/api/v1/auth/me")).json(),
  ).toMatchObject({
    restriction: null,
  });

  const stale = await browser.newContext();
  await stale.addCookies(oldSession);
  expect((await stale.request.get("/api/v1/auth/me")).status()).toBe(401);

  const fresh = await browser.newContext();
  const refused = await apiLogin(fresh, INVITEE.username, temporary);
  expect(refused.status()).toBe(401);
  expect(
    ((await refused.json()) as { error: { code: string } }).error.code,
  ).toBe("AUTH_INVALID_CREDENTIALS");
  expect((await apiLogin(fresh, INVITEE.username, chosen)).status()).toBe(200);

  await requireTwoFactor(origin, true);
  const mandatoryTemporary = await operatorResetPassword(origin, user.id);
  const mandatoryContext = await browser.newContext();
  const enrolling = await mandatoryContext.newPage();
  const enrollingErrors = collectPageErrors(enrolling);
  expect(
    (
      await submitPasswordLogin(enrolling, INVITEE.username, mandatoryTemporary)
    ).status(),
  ).toBe(200);
  await expect(enrolling).toHaveURL(/\/login\/forced-password-change/);
  await expect(enrolling.getByTestId("app-shell")).toHaveCount(0);

  const restrictionsSeen: (string | null)[] = [];
  enrolling.on("response", async (response) => {
    if (
      new URL(response.url()).pathname === "/api/v1/auth/me" &&
      response.ok()
    ) {
      const body = (await response.json()) as { restriction: string | null };
      restrictionsSeen.push(body.restriction);
    }
  });
  const mandatoryPassword = "a second password after the policy change";
  const changed = enrolling.waitForResponse(
    (candidate) =>
      new URL(candidate.url()).pathname === "/api/v1/profile/password" &&
      candidate.request().method() === "POST",
  );
  await enrolling
    .getByLabel("New password", { exact: true })
    .fill(mandatoryPassword);
  await enrolling.getByLabel("Confirm new password").fill(mandatoryPassword);
  await enrolling.getByRole("button", { name: "Set new password" }).click();
  expect((await changed).status()).toBe(204);

  await expect(enrolling).toHaveURL(/\/login\/enroll-2fa/);
  await expect(
    enrolling.getByRole("heading", {
      level: 1,
      name: "Set up two-factor authentication",
    }),
  ).toBeVisible();
  await expect(enrolling.getByTestId("app-shell")).toHaveCount(0);
  await expect(
    enrolling.getByRole("button", { name: "Sign out" }),
  ).toBeVisible();
  expect(restrictionsSeen).toContain("mfa_enrollment_required");

  await expect(enrolling.getByTestId("two-factor-secret")).toBeVisible();
  totp.secret = (
    (await enrolling.getByTestId("two-factor-secret").textContent()) ?? ""
  ).replace(/\s/g, "");
  totp.lastStep = 0;
  expect(totp.secret).toMatch(/^[A-Z2-7]{16,}$/);
  const verifyBodies: unknown[] = [];
  enrolling.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/v1/auth/2fa/enroll/verify"
    ) {
      verifyBodies.push(request.postDataJSON());
    }
  });
  await enrolling.getByLabel("Authentication code").fill(await freshTotp());
  await enrolling.getByRole("button", { name: "Verify and turn on" }).click();
  await expect(enrolling.getByTestId("backup-code")).toHaveCount(10);
  expect(verifyBodies).toHaveLength(1);
  expect(Object.keys(verifyBodies[0] as object).sort()).toEqual([
    "code",
    "enrollmentId",
  ]);
  expect(JSON.stringify(verifyBodies)).not.toContain(totp.secret);
  await enrolling
    .getByRole("checkbox", {
      name: "I saved these backup codes somewhere safe",
    })
    .check();
  await enrolling.getByRole("button", { name: "Continue" }).click();

  await expect(enrolling.getByTestId("app-shell")).toBeVisible();
  await expect(enrolling).toHaveURL(/\/overview$/);
  expect(
    await (await enrolling.request.get("/api/v1/auth/me")).json(),
  ).toMatchObject({
    restriction: null,
    capabilities: { twoFactorEnabled: true },
  });
  totp.secret = "";

  expect(pageErrors).toEqual([]);
  expect(enrollingErrors).toEqual([]);
  await Promise.all([
    context.close(),
    stale.close(),
    fresh.close(),
    mandatoryContext.close(),
  ]);
});
