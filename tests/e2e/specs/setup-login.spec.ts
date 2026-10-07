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
import {
  authorizations,
  denyNextAuthorization,
  IDP_ORIGIN,
  resetIdp,
  setIdentity,
} from "../support/idp";
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
  await login.getByRole("button", { name: /^(?:loading\s+)?Sign in$/ }).click();
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

const MARGARET = {
  firstName: "Margaret",
  lastName: "Hamilton",
  username: "margaret",
  email: "margaret@example.test",
  temporaryPassword: "a temporary passphrase 1",
  chosenPassword: "the passphrase margaret chose",
};

test("e2e_admin_create_user_and_forced_change", async ({ browser }) => {
  test.setTimeout(120_000);
  const adminContext = await browser.newContext();
  const adminPage = await adminContext.newPage();
  const adminErrors = collectPageErrors(adminPage);
  await signIn(adminPage, ADMIN.username, NEW_PASSWORD);

  await adminPage.getByRole("link", { name: "Admin", exact: true }).click();
  await expect(adminPage).toHaveURL(/\/admin\/users$/);
  await expect(
    adminPage.getByRole("heading", { level: 1, name: "Administration" }),
  ).toBeVisible();
  const sections = adminPage.getByRole("navigation", {
    name: "Administration sections",
  });
  await expect(sections.getByRole("link")).toHaveText([
    "Users",
    "Security",
    "SMTP",
    "Providers",
  ]);

  await adminPage.getByRole("button", { name: "Create user" }).click();
  const dialog = adminPage
    .getByRole("dialog")
    .filter({ hasText: "Create user" });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("First name").fill(MARGARET.firstName);
  await dialog.getByLabel("Last name").fill(MARGARET.lastName);
  await dialog.getByLabel("Username").fill(MARGARET.username);
  await dialog.getByLabel("E-mail address").fill(MARGARET.email);
  await dialog
    .getByLabel("Temporary password")
    .fill(MARGARET.temporaryPassword);
  await expect(
    dialog.getByRole("switch", { name: /Require a new password/ }),
  ).toBeChecked();
  const language = dialog.getByRole("combobox", { name: "Language" });
  await language.click();
  await language.fill("English");
  await adminPage
    .getByTitle("English (United States)", { exact: true })
    .filter({ visible: true })
    .click();

  const createBodies: unknown[] = [];
  adminPage.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/v1/admin/users"
    ) {
      createBodies.push(request.postDataJSON());
    }
  });
  await dialog.getByRole("button", { name: "Create user" }).click();
  await completeRecentAuthIfAsked(
    adminPage,
    adminPage.getByText("Margaret Hamilton was created."),
    NEW_PASSWORD,
  );
  expect(createBodies).toHaveLength(1);
  expect(createBodies[0]).toMatchObject({
    username: MARGARET.username,
    email: MARGARET.email,
    role: "user",
    locale: "en-US",
    requirePasswordChange: true,
  });
  await expect(
    adminPage.getByRole("link", { name: "Margaret Hamilton" }),
  ).toBeVisible();
  const listed = (await (
    await adminPage.request.get("/api/v1/admin/users?q=margaret")
  ).json()) as {
    items: { username: string; mustChangePassword: boolean; role: string }[];
  };
  expect(listed.items).toMatchObject([
    { username: MARGARET.username, mustChangePassword: true, role: "user" },
  ]);
  expect(JSON.stringify(listed)).not.toContain(MARGARET.temporaryPassword);

  await adminPage.getByRole("button", { name: "Account menu" }).click();
  await adminPage.getByRole("menuitem", { name: "Sign out" }).click();
  await expect(adminPage).toHaveURL(/\/login(\?|$)/);
  expect(await sessionStatus(adminPage)).toBe(401);

  const userContext = await browser.newContext();
  const page = await userContext.newPage();
  const userErrors = collectPageErrors(page);
  expect(
    (
      await submitPasswordLogin(
        page,
        MARGARET.username,
        MARGARET.temporaryPassword,
      )
    ).status(),
  ).toBe(200);
  await expect(page).toHaveURL(/\/login\/forced-password-change/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Set a new password" }),
  ).toBeVisible();
  await expect(page.getByTestId("app-shell")).toHaveCount(0);
  expect(
    (
      (await (await page.request.get("/api/v1/profile")).json()) as {
        error: { code: string };
      }
    ).error.code,
  ).toBe("AUTH_PASSWORD_CHANGE_REQUIRED");

  await page
    .getByLabel("New password", { exact: true })
    .fill(MARGARET.chosenPassword);
  await page.getByLabel("Confirm new password").fill(MARGARET.chosenPassword);
  await page.getByRole("button", { name: "Set new password" }).click();

  await expect(page).toHaveURL(/\/overview$/);
  await expect(page.getByTestId("app-shell")).toBeVisible();
  expect(
    await (await page.request.get("/api/v1/auth/me")).json(),
  ).toMatchObject({
    user: { username: MARGARET.username, role: "user" },
    restriction: null,
  });
  expect((await page.request.get("/api/v1/profile")).status()).toBe(200);
  expect((await page.request.get("/api/v1/admin/users")).status()).toBe(403);
  await expect(
    page.getByRole("link", { name: "Admin", exact: true }),
  ).toHaveCount(0);
  await page.goto("/admin/users");
  await expect(
    page.getByRole("heading", { level: 1, name: "Access denied" }),
  ).toBeVisible();

  const fresh = await browser.newContext();
  expect(
    (
      await apiLogin(fresh, MARGARET.username, MARGARET.temporaryPassword)
    ).status(),
  ).toBe(401);
  expect(
    (
      await apiLogin(fresh, MARGARET.username, MARGARET.chosenPassword)
    ).status(),
  ).toBe(200);

  expect(adminErrors).toEqual([]);
  expect(userErrors).toEqual([]);
  await Promise.all([adminContext.close(), userContext.close(), fresh.close()]);
});

const MARGARET_NEW_EMAIL = "margaret.hamilton@example.test";

test("e2e_admin_email_change_verification_link", async ({
  browser,
  baseURL,
}) => {
  test.setTimeout(180_000);
  const origin = baseURL ?? APP_ORIGIN;
  await configureSmtpSink(origin);
  await clearSink();

  const adminContext = await browser.newContext();
  const adminPage = await adminContext.newPage();
  const adminErrors = collectPageErrors(adminPage);
  await signIn(adminPage, ADMIN.username, NEW_PASSWORD);
  await adminPage.goto("/admin/users");
  await adminPage.getByRole("link", { name: "Margaret Hamilton" }).click();
  const email = adminPage.getByTestId("user-email");
  await expect(email.getByTestId("canonical-email")).toHaveText(MARGARET.email);
  await email.getByLabel("New e-mail address").fill(MARGARET_NEW_EMAIL);
  await email.getByRole("button", { name: "Start e-mail change" }).click();
  await completeRecentAuthIfAsked(
    adminPage,
    adminPage.getByText(
      "Verification requested. The address changes only after it is confirmed.",
    ),
    NEW_PASSWORD,
  );
  await expect(email.getByTestId("pending-email-notice")).toContainText(
    MARGARET_NEW_EMAIL,
  );
  await expect(email.getByTestId("canonical-email")).toHaveText(MARGARET.email);

  await runJobsOnce(origin, "email.send");
  await expect
    .poll(async () => (await messagesTo(MARGARET_NEW_EMAIL)).length)
    .toBe(1);
  const [message] = await messagesTo(MARGARET_NEW_EMAIL);
  const link = /https?:\/\/[^\s"'<>]+\/verify-email\/[A-Za-z0-9_%-]+/.exec(
    message?.Text ?? "",
  )?.[0];
  expect(link).toBeDefined();
  const verifyUrl = new URL(link ?? "");
  expect(verifyUrl.origin).toBe(new URL(origin).origin);
  expect(verifyUrl.search).toBe("");
  expect(verifyUrl.pathname).toMatch(/^\/verify-email\/[A-Za-z0-9_%-]+$/);

  const userContext = await browser.newContext();
  const page = await userContext.newPage();
  const pageErrors = collectPageErrors(page);
  await signIn(page, MARGARET.username, MARGARET.chosenPassword);
  expect(await sessionStatus(page)).toBe(200);

  const verifyPosts: unknown[] = [];
  page.on("request", (request) => {
    if (
      request.method() === "POST" &&
      new URL(request.url()).pathname === "/api/v1/auth/email/verify"
    ) {
      verifyPosts.push(request.postDataJSON());
    }
  });
  await page.goto(link ?? "");
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "Confirm your new e-mail address",
    }),
  ).toBeVisible();
  await expect(page.getByText("Page not found")).toHaveCount(0);
  expect(verifyPosts).toEqual([]);
  await page.getByRole("button", { name: "Confirm new address" }).click();

  await expect(page.getByTestId("verify-email-success")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 1, name: "E-mail address confirmed" }),
  ).toBeVisible();
  const token = verifyUrl.pathname.split("/").pop() ?? "";
  expect(verifyPosts).toEqual([{ token: decodeURIComponent(token) }]);
  await expect(page.locator("body")).not.toContainText(
    decodeURIComponent(token),
  );
  expect(await sessionStatus(page)).toBe(401);
  const storage = await page.evaluate(() => ({
    local: JSON.stringify(window.localStorage),
    session: JSON.stringify(window.sessionStorage),
    cookie: document.cookie,
  }));
  expect(JSON.stringify(storage)).not.toContain(decodeURIComponent(token));

  await page.getByRole("button", { name: "Go to sign in" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(
    page.getByRole("heading", { level: 1, name: "Sign in" }),
  ).toBeVisible();

  const fresh = await browser.newContext();
  expect(
    (await apiLogin(fresh, MARGARET.email, MARGARET.chosenPassword)).status(),
  ).toBe(401);
  expect(
    (
      await apiLogin(fresh, MARGARET.username, MARGARET.chosenPassword)
    ).status(),
  ).toBe(200);
  const me = (await (await fresh.request.get("/api/v1/auth/me")).json()) as {
    user: { email: string; pendingEmail: string | null };
  };
  expect(me.user.email).toBe(MARGARET_NEW_EMAIL);
  expect(me.user.pendingEmail).toBeNull();
  const byNewAddress = await browser.newContext();
  expect(
    (
      await apiLogin(byNewAddress, MARGARET_NEW_EMAIL, MARGARET.chosenPassword)
    ).status(),
  ).toBe(200);

  await adminPage.reload();
  await expect(
    adminPage.getByTestId("user-email").getByTestId("canonical-email"),
  ).toHaveText(MARGARET_NEW_EMAIL);
  await expect(
    adminPage.getByTestId("user-email").getByTestId("pending-email-notice"),
  ).toHaveCount(0);

  expect(adminErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
  await Promise.all([
    adminContext.close(),
    userContext.close(),
    fresh.close(),
    byNewAddress.close(),
  ]);
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

const SSO_CLIENT = { id: "palmr-e2e", secret: "mock-client-secret" };
const FIRST_PROVIDER = { slug: "mockidp", displayName: "Mock IdP" };
const SECOND_PROVIDER = { slug: "mockidp2", displayName: "Mock IdP Two" };
const ALICE_AT_IDP = {
  sub: "idp-alice",
  email: "alice@idp.example.test",
  email_verified: true,
  name: "Alice Idp",
  preferred_username: "alice",
};
const ADA_AT_IDP = {
  sub: "idp-ada",
  email: "ada@idp.example.test",
  email_verified: true,
  name: "Ada Idp",
  preferred_username: "ada-idp",
};

async function adminApi(
  context: BrowserContext,
  method: "PATCH" | "POST",
  path: string,
  data?: unknown,
) {
  const csrf = await csrfOf(context);
  return context.request.fetch(path, {
    method,
    headers: { "X-Palmr-CSRF": csrf },
    ...(data === undefined ? {} : { data }),
  });
}

async function adminSession(browser: Browser): Promise<BrowserContext> {
  const context = await browser.newContext();
  const login = await apiLogin(context, ADMIN.username, NEW_PASSWORD);
  expect(login.status()).toBe(200);
  expect(await login.json()).not.toHaveProperty(
    "restriction",
    "mfa_enrollment_required",
  );
  return context;
}

async function configureProvider(
  context: BrowserContext,
  provider: { slug: string; displayName: string },
  autoProvision: boolean,
) {
  const created = await adminApi(context, "POST", "/api/v1/admin/providers", {
    slug: provider.slug,
    displayName: provider.displayName,
    protocol: "oidc",
    preset: "generic",
    issuerUrl: IDP_ORIGIN,
    clientId: SSO_CLIENT.id,
    clientSecret: SSO_CLIENT.secret,
    tokenAuthMethod: "client_secret_basic",
    scopes: ["openid", "email", "profile"],
    autoProvision,
    allowEmailLinking: true,
    enabled: true,
  });
  expect(created.status()).toBe(201);
  const { id } = (await created.json()) as { id: string };
  const tested = await adminApi(
    context,
    "POST",
    `/api/v1/admin/providers/${id}/test`,
  );
  expect(tested.status()).toBe(200);
  expect(await tested.json()).toMatchObject({ ok: true });
}

async function waitForRecentAuthToLapse(page: Page) {
  await expect
    .poll(
      async () => {
        const response = await page.request.get("/api/v1/auth/me");
        const serverNow = Date.parse(response.headers()["date"] ?? "");
        const me = (await response.json()) as {
          session: { recentAuthUntil: string };
        };
        return Date.parse(me.session.recentAuthUntil) + 2_000 < serverNow;
      },
      { timeout: 150_000, intervals: [2_000] },
    )
    .toBe(true);
}

async function replayStateAbsent(page: Page) {
  return page.evaluate(async () => ({
    local: window.localStorage.length,
    session: window.sessionStorage.length,
    databases: (await window.indexedDB.databases()).length,
    search: window.location.search,
    hash: window.location.hash,
  }));
}

test.describe("external identity", () => {
  test.describe.configure({ mode: "serial" });

  let aliceContext: BrowserContext | null = null;
  let linkSession: BrowserContext | null = null;
  let unlinkSession: BrowserContext | null = null;

  test.beforeAll(async ({ browser, baseURL }, testInfo) => {
    testInfo.setTimeout(240_000);
    const origin = baseURL ?? APP_ORIGIN;
    await requireTwoFactor(origin, false);
    await resetIdp();
    const admin = await adminSession(browser);
    const lowered = await adminApi(
      admin,
      "PATCH",
      "/api/v1/admin/settings/security",
      { recentAuthMinutes: 1 },
    );
    expect(lowered.status()).toBe(200);
    await configureProvider(admin, FIRST_PROVIDER, true);
    await configureProvider(admin, SECOND_PROVIDER, false);
    await admin.close();
    linkSession = await adminSession(browser);
    unlinkSession = await adminSession(browser);
  });

  test.afterAll(async ({ browser }, testInfo) => {
    testInfo.setTimeout(120_000);
    await Promise.all([
      aliceContext?.close(),
      linkSession?.close(),
      unlinkSession?.close(),
    ]);
    const admin = await adminSession(browser);
    const restored = await adminApi(
      admin,
      "PATCH",
      "/api/v1/admin/settings/security",
      { recentAuthMinutes: 5 },
    );
    expect(restored.status()).toBe(200);
    await admin.close();
  });

  test("e2e_oidc_login_mock_idp", async ({ browser, baseURL }) => {
    test.setTimeout(120_000);
    const origin = baseURL ?? APP_ORIGIN;

    const refused = await browser.newContext();
    const refusedPage = await refused.newPage();
    const refusedErrors = collectPageErrors(refusedPage);
    await refusedPage.goto("/login");
    const providers = refusedPage.getByTestId("login-providers");
    await expect(providers.getByRole("button")).toHaveText([
      "Continue with Mock IdP",
      "Continue with Mock IdP Two",
    ]);
    await denyNextAuthorization();
    await providers
      .getByRole("button", { name: "Continue with Mock IdP", exact: true })
      .click();
    const callbackError = refusedPage.getByTestId("login-callback-error");
    await expect(callbackError).toContainText(
      "The identity provider denied the sign-in request.",
    );
    await expect(callbackError).toContainText(/Request ID: [A-Za-z0-9._-]+/);
    await expect(callbackError).not.toContainText("The mock user denied");
    await expect(refusedPage).toHaveURL(/\/login$/);
    expect(await refusedPage.evaluate(() => window.location.search)).toBe("");
    await refusedPage.reload();
    await expect(refusedPage.getByTestId("login-callback-error")).toHaveCount(
      0,
    );
    expect(refusedErrors).toEqual([]);
    await refused.close();

    await setIdentity(ALICE_AT_IDP);
    const context = await browser.newContext();
    const page = await context.newPage();
    const pageErrors = collectPageErrors(page);
    await page.goto("/login");
    await page
      .getByRole("button", { name: "Continue with Mock IdP", exact: true })
      .click();
    await expect(page).toHaveURL(/\/overview$/);
    await expect(page.getByTestId("app-shell")).toBeVisible();
    const me = await page.request.get("/api/v1/auth/me");
    expect(me.status()).toBe(200);
    expect(await me.json()).toMatchObject({
      user: { username: "alice", email: ALICE_AT_IDP.email, role: "user" },
      capabilities: { hasLocalPassword: false, identityLinkCount: 1 },
      restriction: null,
    });
    const requests = await authorizations();
    const login = requests.at(-1);
    expect(login).toMatchObject({
      clientId: SSO_CLIENT.id,
      challengeMethod: "S256",
      hasNonce: true,
      redirectUri: `${origin}/api/v1/auth/providers/${FIRST_PROVIDER.slug}/callback`,
    });

    const english = await page.request.fetch("/api/v1/profile/preferences", {
      method: "PATCH",
      headers: { "X-Palmr-CSRF": await csrfOf(context) },
      data: { locale: "en-US" },
    });
    expect(english.status()).toBe(200);
    await page.goto("/settings/security");
    const section = page.getByTestId("settings-identity-links");
    const row = section.getByTestId("identity-link-row");
    await expect(row).toHaveCount(1);
    await expect(row).toContainText("Mock IdP");
    await expect(row).toContainText(`Linked as ${ALICE_AT_IDP.email}`);
    await expect(row).not.toContainText(ALICE_AT_IDP.sub);

    await row.getByRole("button", { name: "Remove Mock IdP" }).click();
    await page
      .getByRole("dialog", { name: "Remove Mock IdP?" })
      .getByRole("button", { name: "Remove account" })
      .click();
    await expect(
      section.getByText(
        "This is the only way to sign in to this account, so it can't be removed.",
      ),
    ).toBeVisible();
    await expect(row).toHaveCount(1);
    expect((await page.request.get("/api/v1/auth/me")).status()).toBe(200);

    expect(pageErrors).toEqual([]);
    aliceContext = context;
  });

  test("e2e_admin_providers_configure_via_ui", async ({ browser }) => {
    test.setTimeout(180_000);
    const context = await adminSession(browser);
    const page = await context.newPage();
    const pageErrors = collectPageErrors(page);
    const secretSeen: string[] = [];
    page.on("response", async (response) => {
      if (
        new URL(response.url()).pathname.startsWith("/api/v1/admin/providers")
      ) {
        try {
          secretSeen.push(await response.text());
        } catch {
          return;
        }
      }
    });

    await page.goto("/admin/providers");
    const rows = page.getByTestId("provider-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.first()).toContainText("Mock IdP");
    await expect(rows.first().getByTestId("provider-validation")).toContainText(
      "Tested",
    );
    await expect(
      rows
        .first()
        .getByTestId("provider-redirect-uri")
        .locator("[data-redirect-uri]"),
    ).toHaveAttribute(
      "data-redirect-uri",
      `${APP_ORIGIN}/api/v1/auth/providers/${FIRST_PROVIDER.slug}/callback`,
    );

    await page
      .getByRole("button", { name: "Add provider", exact: true })
      .click();
    const form = page.getByTestId("provider-form");
    await form.getByLabel("Provider type").click();
    await page.getByTitle("Custom OpenID Connect", { exact: true }).click();
    await form.getByLabel("Display name").fill("UI IdP");
    await form.getByLabel("Slug").fill("uiidp");
    await form.getByLabel("Issuer URL").fill(IDP_ORIGIN);
    await form.getByRole("button", { name: "Discover settings" }).click();
    const discovery = form.getByTestId("provider-discovery");
    await expect(discovery).toContainText(`${IDP_ORIGIN}/token`);
    await discovery.getByRole("button", { name: "Use these values" }).click();
    await form.getByLabel("Client ID", { exact: true }).fill(SSO_CLIENT.id);
    await form.getByLabel("Client secret").fill(SSO_CLIENT.secret);
    await form.getByRole("button", { name: "Add provider" }).click();
    await completeRecentAuthIfAsked(
      page,
      page.getByTestId("provider-row").filter({ hasText: "UI IdP" }),
      NEW_PASSWORD,
    );
    await expect(rows).toHaveCount(3);

    const created = rows.filter({ hasText: "UI IdP" });
    await expect(created.getByTestId("provider-enabled")).toHaveAttribute(
      "data-enabled",
      "false",
    );
    await expect(created.getByTestId("provider-secret")).toHaveAttribute(
      "data-configured",
      "true",
    );
    await created.getByRole("button", { name: "Test UI IdP" }).click();
    await expect(created.getByTestId("provider-checks")).toContainText(
      "Passed",
    );
    await expect(created.getByTestId("provider-validation")).toHaveAttribute(
      "data-state",
      "validated",
    );

    await created.getByRole("button", { name: "Move UI IdP up" }).click();
    await expect
      .poll(async () =>
        rows.evaluateAll((all) =>
          all.map((row) => row.getAttribute("data-provider-slug")),
        ),
      )
      .toEqual([FIRST_PROVIDER.slug, "uiidp", SECOND_PROVIDER.slug]);
    await created.getByRole("switch", { name: "Enable UI IdP" }).click();
    await expect(created.getByTestId("provider-enabled")).toHaveAttribute(
      "data-enabled",
      "true",
    );
    await expect
      .poll(async () => {
        const bootstrap = (await (
          await page.request.get("/api/v1/bootstrap")
        ).json()) as { providers: { slug: string }[] };
        return bootstrap.providers.map((provider) => provider.slug);
      })
      .toEqual([FIRST_PROVIDER.slug, "uiidp", SECOND_PROVIDER.slug]);

    await created.getByRole("button", { name: "Edit UI IdP" }).click();
    const editor = page.getByTestId("provider-form");
    await expect(editor.getByLabel("Client secret")).toHaveValue("");
    await expect(
      editor.getByTestId("provider-form-secret-state"),
    ).toHaveAttribute("data-configured", "true");
    expect(await page.content()).not.toContain(SSO_CLIENT.secret);
    expect(secretSeen.join("\n")).not.toContain(SSO_CLIENT.secret);
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByTestId("provider-form")).toHaveCount(0);

    await created.getByRole("button", { name: "Delete UI IdP" }).click();
    const confirm = page.getByRole("dialog", { name: "Delete UI IdP?" });
    await confirm.getByRole("button", { name: "Delete provider" }).click();
    await completeRecentAuthIfAsked(
      page,
      page.getByText("Provider deleted."),
      NEW_PASSWORD,
    );
    await expect(rows).toHaveCount(2);

    expect(pageErrors).toEqual([]);
    await context.close();
  });

  test("e2e_link_unlink_requires_recent_auth", async ({ browser }) => {
    test.setTimeout(240_000);
    expect(linkSession).not.toBeNull();
    expect(unlinkSession).not.toBeNull();
    await setIdentity(ADA_AT_IDP);
    const page = await (linkSession as BrowserContext).newPage();
    const pageErrors = collectPageErrors(page);

    const linkRequests: number[] = [];
    page.on("response", (response) => {
      if (
        response.request().method() === "POST" &&
        new URL(response.url()).pathname ===
          `/api/v1/auth/providers/${FIRST_PROVIDER.slug}/link`
      ) {
        linkRequests.push(response.status());
      }
    });

    await waitForRecentAuthToLapse(page);
    await page.goto("/settings/security");
    const section = page.getByTestId("settings-identity-links");
    await expect(section.getByTestId("identity-links-empty")).toBeVisible();
    await section
      .getByRole("button", { name: "Connect Mock IdP", exact: true })
      .click();
    const challenge = page
      .getByRole("dialog")
      .filter({ hasText: "Confirm it's you" });
    await expect(challenge).toBeVisible();
    await challenge.getByLabel("Password").fill(NEW_PASSWORD);
    await challenge.getByRole("button", { name: "Confirm" }).click();

    const linked = page
      .getByTestId("settings-identity-links")
      .getByTestId("identity-link-row");
    await expect(linked).toHaveCount(1, { timeout: 30_000 });
    await expect(page).toHaveURL(/\/settings\/security$/);
    await expect(linked).toContainText("Mock IdP");
    await expect(linked).toContainText(`Linked as ${ADA_AT_IDP.email}`);
    expect(linkRequests).toEqual([403, 200]);

    const other = await (unlinkSession as BrowserContext).newPage();
    const otherErrors = collectPageErrors(other);
    await waitForRecentAuthToLapse(other);
    await other.goto("/settings/security");
    const row = other
      .getByTestId("settings-identity-links")
      .getByTestId("identity-link-row");
    await expect(row).toHaveCount(1);
    await row.getByRole("button", { name: "Remove Mock IdP" }).click();
    await other
      .getByRole("dialog", { name: "Remove Mock IdP?" })
      .getByRole("button", { name: "Remove account" })
      .click();
    const unlinkChallenge = other
      .getByRole("dialog")
      .filter({ hasText: "Confirm it's you" });
    await expect(unlinkChallenge).toBeVisible();
    await unlinkChallenge.getByLabel("Password").fill(NEW_PASSWORD);
    await unlinkChallenge.getByRole("button", { name: "Confirm" }).click();

    await expect(
      other.getByRole("heading", { level: 1, name: "Sign in" }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(other.getByTestId("login-notice")).toContainText(
      "Sign-in method removed",
    );
    expect((await other.request.get("/api/v1/auth/me")).status()).toBe(401);
    expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);

    const fresh = await browser.newContext();
    expect((await apiLogin(fresh, ADMIN.username, NEW_PASSWORD)).status()).toBe(
      200,
    );
    const links = await fresh.request.get("/api/v1/identity-links");
    expect(await links.json()).toMatchObject({ items: [], totalCount: 0 });

    expect(pageErrors).toEqual([]);
    expect(otherErrors).toEqual([]);
    await fresh.close();
  });

  test("e2e_sso_recent_auth_replays_sensitive_mutation", async () => {
    test.setTimeout(240_000);
    expect(aliceContext).not.toBeNull();
    const context = aliceContext as BrowserContext;
    const page = context.pages()[0] as Page;
    const pageErrors = collectPageErrors(page);
    await setIdentity(ALICE_AT_IDP);
    await waitForRecentAuthToLapse(page);

    const openerSevered: boolean[] = [];
    await context.exposeFunction(
      "__reportOpenerSevered",
      (severed: boolean) => {
        openerSevered.push(severed);
      },
    );
    await context.addInitScript(() => {
      if (location.pathname === "/auth/reauth-complete") {
        (
          window as unknown as {
            __reportOpenerSevered: (severed: boolean) => void;
          }
        ).__reportOpenerSevered(window.opener === null);
      }
    });

    const timeline: string[] = [];
    const linkResponses: number[] = [];
    const reauthResponses: { channel: string; url: string }[] = [];
    page.on("response", (response) => {
      const { pathname } = new URL(response.url());
      const method = response.request().method();
      if (
        method === "POST" &&
        pathname === `/api/v1/auth/providers/${SECOND_PROVIDER.slug}/link`
      ) {
        linkResponses.push(response.status());
        timeline.push(`link:${String(response.status())}`);
      }
      if (method === "GET" && pathname === "/api/v1/auth/me") {
        timeline.push("me");
      }
    });
    const reauthRequests: unknown[] = [];
    page.on("request", (request) => {
      if (
        request.method() === "POST" &&
        new URL(request.url()).pathname === "/api/v1/auth/reauthenticate"
      ) {
        reauthRequests.push(request.postDataJSON());
        timeline.push("reauth");
      }
    });
    page.on("response", (response) => {
      if (
        response.request().method() === "POST" &&
        new URL(response.url()).pathname === "/api/v1/auth/reauthenticate"
      ) {
        void response.json().then((body: unknown) => {
          const { externalReauthUrl, externalReauthChannel } = body as {
            externalReauthUrl: string;
            externalReauthChannel: string;
          };
          reauthResponses.push({
            url: externalReauthUrl,
            channel: externalReauthChannel,
          });
        });
      }
    });

    await page.goto("/settings/security");
    const section = page.getByTestId("settings-identity-links");
    await expect(section.getByTestId("identity-link-row")).toHaveCount(1);
    await section
      .getByRole("button", { name: "Connect Mock IdP Two", exact: true })
      .click();

    const challenge = page
      .getByRole("dialog")
      .filter({ hasText: "Confirm it's you" });
    await expect(challenge).toBeVisible();
    await expect(challenge.getByLabel("Password")).toHaveCount(0);
    await expect(challenge.getByLabel("Authentication code")).toHaveCount(0);
    expect(await replayStateAbsent(page)).toEqual({
      local: 0,
      session: 0,
      databases: 0,
      search: "",
      hash: "",
    });

    const popupOpened = page.waitForEvent("popup");
    await challenge
      .getByRole("button", { name: /Continue with your sign-in provider/ })
      .click();
    const popup = await popupOpened;
    const popupUrls: string[] = [];
    popup.on("framenavigated", (frame) => {
      if (frame === popup.mainFrame()) {
        popupUrls.push(frame.url());
        if (frame.url().includes("/auth/reauth-complete")) {
          timeline.push("complete");
        }
      }
    });
    await popup.waitForEvent("close", { timeout: 60_000 });
    expect(reauthResponses).toHaveLength(1);
    const [{ channel }] = reauthResponses as [{ channel: string; url: string }];
    expect(channel).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(popupUrls).toContain(
      `${new URL(page.url()).origin}/auth/reauth-complete?status=success&channel=${channel}`,
    );
    expect(openerSevered).toEqual([true]);

    await expect(section.getByTestId("identity-link-row")).toHaveCount(2, {
      timeout: 30_000,
    });
    await expect(page).toHaveURL(/\/settings\/security$/);
    await expect(
      section
        .getByTestId("identity-link-row")
        .filter({ hasText: "Mock IdP Two" }),
    ).toHaveCount(1);
    expect(reauthRequests).toEqual([{}]);
    expect(linkResponses).toEqual([403, 200]);
    const completed = timeline.indexOf("complete");
    const replayed = timeline.lastIndexOf("link:200");
    expect(timeline.indexOf("link:403")).toBeLessThan(
      timeline.indexOf("reauth"),
    );
    expect(timeline.indexOf("reauth")).toBeLessThan(completed);
    expect(timeline.slice(completed, replayed)).toContain("me");
    expect(timeline.filter((entry) => entry === "link:200")).toHaveLength(1);
    const reauthorization = (await authorizations()).find(
      (request) => request.prompt === "login",
    );
    expect(reauthorization).toMatchObject({
      maxAge: "0",
      challengeMethod: "S256",
      hasNonce: true,
    });
    expect(await replayStateAbsent(page)).toEqual({
      local: 0,
      session: 0,
      databases: 0,
      search: "",
      hash: "",
    });
    expect(
      (await context.cookies()).map((cookie) => cookie.name).sort(),
    ).toEqual(expect.not.arrayContaining(["palmr_replay", "palmr_reauth"]));
    expect(pageErrors).toEqual([]);
  });
});
