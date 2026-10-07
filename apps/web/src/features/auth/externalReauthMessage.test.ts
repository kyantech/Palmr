import { describe, expect, test } from "vitest";
import {
  EXTERNAL_REAUTH_MESSAGE_TYPE,
  messageForLanding,
  parseExternalReauthMessage,
  readExternalReauthCompletion,
} from "./externalReauthMessage";

const CHANNEL = "AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s";

describe("unit_external_reauth_landing", () => {
  test("a success landing carries the channel and nothing else", () => {
    expect(readExternalReauthCompletion(`?status=success&channel=${CHANNEL}`)).toEqual({
      channel: CHANNEL,
      landing: { status: "success" },
    });
    expect(
      readExternalReauthCompletion(
        `?status=success&channel=${CHANNEL}&error=PROVIDER_STATE_INVALID&token=t`,
      ),
    ).toEqual({ channel: CHANNEL, landing: { status: "success" } });
  });

  test("an error landing carries the structured code, the callback request id and the channel", () => {
    expect(
      readExternalReauthCompletion(
        `?status=error&error=PROVIDER_AUTH_DENIED&requestId=req-1&channel=${CHANNEL}`,
      ),
    ).toEqual({
      channel: CHANNEL,
      landing: { status: "error", reported: { code: "PROVIDER_AUTH_DENIED", requestId: "req-1" } },
    });
    expect(
      readExternalReauthCompletion(`?status=error&error=AUTH_LOCKED&channel=${CHANNEL}`),
    ).toEqual({
      channel: CHANNEL,
      landing: { status: "error", reported: { code: "AUTH_LOCKED", requestId: null } },
    });
  });

  test.each([
    "",
    "?status=",
    "?status=ok",
    "?status=error",
    "?error=PROVIDER_AUTH_DENIED",
    `?status=error&error=not-a-code&channel=${CHANNEL}`,
    `?status=error&error=%3Cscript%3E&channel=${CHANNEL}`,
    `?status=error&error=error_description&channel=${CHANNEL}`,
    `?status=error&error=${"A".repeat(80)}&channel=${CHANNEL}`,
  ])("%s is not a landing", (search) => {
    expect(readExternalReauthCompletion(search)).toBeNull();
  });

  test.each([
    ["is missing", ""],
    ["is empty", "&channel="],
    ["is too short", "&channel=abc"],
    ["is too long", `&channel=${CHANNEL}A`],
    ["has characters outside base64url", `&channel=${CHANNEL.slice(0, 42)}!`],
    ["has a path separator", `&channel=${CHANNEL.slice(0, 42)}/`],
    ["is padded", `&channel=${CHANNEL.slice(0, 42)}=`],
  ])("a landing whose channel %s is refused, so nothing is ever broadcast", (_label, suffix) => {
    expect(readExternalReauthCompletion(`?status=success${suffix}`)).toBeNull();
    expect(readExternalReauthCompletion(`?status=error&error=AUTH_LOCKED${suffix}`)).toBeNull();
  });

  test("an unsafe request id is dropped, never rendered", () => {
    expect(
      readExternalReauthCompletion(
        `?status=error&error=AUTH_LOCKED&requestId=%3Cb%3Eboom%3C%2Fb%3E&channel=${CHANNEL}`,
      ),
    ).toEqual({
      channel: CHANNEL,
      landing: { status: "error", reported: { code: "AUTH_LOCKED", requestId: null } },
    });
  });
});

describe("unit_external_reauth_message", () => {
  test("the outgoing messages have exactly the accepted shapes", () => {
    expect(messageForLanding({ status: "success" })).toEqual({
      type: "palmr:external-reauth",
      status: "success",
    });
    expect(
      messageForLanding({ status: "error", reported: { code: "AUTH_LOCKED", requestId: "req-2" } }),
    ).toEqual({
      type: "palmr:external-reauth",
      status: "error",
      error: "AUTH_LOCKED",
      requestId: "req-2",
    });
    expect(EXTERNAL_REAUTH_MESSAGE_TYPE).toBe("palmr:external-reauth");
  });

  test("a message round-trips through the parser", () => {
    const success = messageForLanding({ status: "success" });
    expect(parseExternalReauthMessage(success)).toEqual({ status: "success" });
    const failure = messageForLanding({
      status: "error",
      reported: { code: "PROVIDER_ID_TOKEN_INVALID", requestId: "req-3" },
    });
    expect(parseExternalReauthMessage(failure)).toEqual({
      status: "error",
      reported: { code: "PROVIDER_ID_TOKEN_INVALID", requestId: "req-3" },
    });
  });

  test.each([
    ["null", null],
    ["a string", "palmr:external-reauth"],
    ["an array", ["palmr:external-reauth", "success"]],
    ["no type", { status: "success" }],
    ["another type", { type: "other", status: "success" }],
    ["an unknown status", { type: "palmr:external-reauth", status: "pending" }],
    ["success with an extra key", { type: "palmr:external-reauth", status: "success", extra: 1 }],
    [
      "success carrying an error",
      { type: "palmr:external-reauth", status: "success", error: "AUTH_LOCKED" },
    ],
    ["an error without a code", { type: "palmr:external-reauth", status: "error" }],
    [
      "an error without a request id key",
      { type: "palmr:external-reauth", status: "error", error: "AUTH_LOCKED" },
    ],
    [
      "an error with a non-string code",
      { type: "palmr:external-reauth", status: "error", error: 7, requestId: null },
    ],
    [
      "an error with a malformed code",
      { type: "palmr:external-reauth", status: "error", error: "bad code", requestId: null },
    ],
    [
      "an error with a malformed request id",
      { type: "palmr:external-reauth", status: "error", error: "AUTH_LOCKED", requestId: "a b" },
    ],
    [
      "an error with an extra key",
      {
        type: "palmr:external-reauth",
        status: "error",
        error: "AUTH_LOCKED",
        requestId: null,
        token: "secret",
      },
    ],
  ])("%s is ignored", (_label, data) => {
    expect(parseExternalReauthMessage(data)).toBeNull();
  });
});
