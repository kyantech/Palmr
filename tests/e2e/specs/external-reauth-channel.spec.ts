import { expect, type Page, test } from "@playwright/test";
import { randomBytes } from "node:crypto";

const IDP_ORIGIN = "https://idp.example.test";
const CHANNEL_PREFIX = "palmr:external-reauth:";

interface Received {
  channel: string;
  data: unknown;
}

interface HarnessWindow {
  __received: Received[];
  __channels: BroadcastChannel[];
  __popup: Window | null;
}

async function listen(page: Page, ids: string[]) {
  await page.evaluate(
    ({ ids: channelIds, prefix }) => {
      const harness = window as unknown as HarnessWindow;
      harness.__received = [];
      harness.__channels = channelIds.map((id) => {
        const channel = new BroadcastChannel(`${prefix}${id}`);
        channel.onmessage = (event: MessageEvent<unknown>) => {
          harness.__received.push({ channel: id, data: event.data });
        };
        return channel;
      });
    },
    { ids, prefix: CHANNEL_PREFIX },
  );
}

async function announce(page: Page, id: string, message: unknown) {
  await page.evaluate(
    ({ id: channelId, message: payload, prefix }) => {
      const channel = new BroadcastChannel(`${prefix}${channelId}`);
      channel.postMessage(payload);
      channel.close();
    },
    { id, message, prefix: CHANNEL_PREFIX },
  );
}

function received(page: Page) {
  return page.evaluate(() => (window as unknown as HarnessWindow).__received);
}

test("e2e_external_reauth_broadcast_channel_survives_coop", async ({
  page,
  context,
  baseURL,
}) => {
  const app = new URL(baseURL as string).origin;
  const channelA = randomBytes(32).toString("base64url");
  const channelB = randomBytes(32).toString("base64url");
  expect(channelA).toHaveLength(43);
  expect(channelA).not.toBe(channelB);

  await context.route(`${IDP_ORIGIN}/**`, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<!doctype html><title>idp</title>",
    }),
  );
  const completionHeaders: Record<string, string>[] = [];
  await context.route(
    (url) => url.origin === app && url.pathname === "/auth/reauth-complete",
    async (route) => {
      const response = await route.fetch();
      const headers = response.headers();
      completionHeaders.push(headers);
      await route.fulfill({
        status: response.status(),
        headers: Object.fromEntries(
          Object.entries(headers).filter(
            ([name]) =>
              name !== "content-length" && name !== "content-encoding",
          ),
        ),
        body: "<!doctype html><title>completion</title>",
      });
    },
  );

  const parentResponse = await page.goto("/");
  expect(parentResponse?.headers()["cross-origin-opener-policy"]).toBe(
    "same-origin",
  );
  await listen(page, [channelA, channelB]);

  const popupOpened = page.waitForEvent("popup");
  await page.evaluate(() => {
    (window as unknown as HarnessWindow).__popup = window.open("about:blank");
  });
  const popup = await popupOpened;

  await popup.evaluate((url) => {
    window.location.replace(url);
  }, `${IDP_ORIGIN}/authorize?prompt=login`);
  await popup.waitForURL(`${IDP_ORIGIN}/**`);

  const completion = `${app}/auth/reauth-complete?status=success&channel=${channelA}`;
  await popup.evaluate((url) => {
    window.location.replace(url);
  }, completion);
  await popup.waitForURL(completion);

  expect(
    await popup.evaluate(() => window.opener === null),
    "COOP same-origin severs the opener once the popup crossed origins",
  ).toBe(true);
  expect(completionHeaders).toHaveLength(1);
  expect(completionHeaders[0]?.["cross-origin-opener-policy"]).toBe(
    "same-origin",
  );

  await announce(popup, channelA, {
    type: "palmr:external-reauth",
    status: "success",
  });
  await expect
    .poll(() => received(page))
    .toEqual([
      {
        channel: channelA,
        data: { type: "palmr:external-reauth", status: "success" },
      },
    ]);

  await announce(popup, channelB, {
    type: "palmr:external-reauth",
    status: "error",
    error: "PROVIDER_AUTH_DENIED",
    requestId: "req-1",
  });
  await expect.poll(async () => (await received(page)).length).toBe(2);
  expect(await received(page)).toEqual([
    {
      channel: channelA,
      data: { type: "palmr:external-reauth", status: "success" },
    },
    {
      channel: channelB,
      data: {
        type: "palmr:external-reauth",
        status: "error",
        error: "PROVIDER_AUTH_DENIED",
        requestId: "req-1",
      },
    },
  ]);

  await popup.close();
});
