import { describe, expect, test } from "vitest";
import {
  EXTERNAL_REAUTH_MESSAGE_TYPE,
  messageForLanding,
  parseExternalReauthMessage,
  readExternalReauthLanding,
} from "./externalReauthMessage";

describe("unit_external_reauth_landing", () => {
  test("a success landing carries nothing else", () => {
    expect(readExternalReauthLanding("?status=success")).toEqual({ status: "success" });
    expect(readExternalReauthLanding("?status=success&error=PROVIDER_STATE_INVALID")).toEqual({
      status: "success",
    });
  });

  test("an error landing carries the structured code and the callback request id", () => {
    expect(
      readExternalReauthLanding("?status=error&error=PROVIDER_AUTH_DENIED&requestId=req-1"),
    ).toEqual({ status: "error", reported: { code: "PROVIDER_AUTH_DENIED", requestId: "req-1" } });
    expect(readExternalReauthLanding("?status=error&error=AUTH_LOCKED")).toEqual({
      status: "error",
      reported: { code: "AUTH_LOCKED", requestId: null },
    });
  });

  test.each([
    "",
    "?status=",
    "?status=ok",
    "?status=error",
    "?status=error&error=not-a-code",
    "?status=error&error=%3Cscript%3E",
    "?status=error&error=error_description",
    `?status=error&error=${"A".repeat(80)}`,
    "?error=PROVIDER_AUTH_DENIED",
  ])("%s is not a landing", (search) => {
    expect(readExternalReauthLanding(search)).toBeNull();
  });

  test("an unsafe request id is dropped, never rendered", () => {
    expect(
      readExternalReauthLanding("?status=error&error=AUTH_LOCKED&requestId=%3Cb%3Eboom%3C%2Fb%3E"),
    ).toEqual({ status: "error", reported: { code: "AUTH_LOCKED", requestId: null } });
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
