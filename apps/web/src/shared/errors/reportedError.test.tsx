import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, test } from "vitest";
import { loadedI18n } from "../../test/renderRouter";
import { ReportedErrorAlert } from "./ErrorView";
import {
  ERROR_PRESENTATION,
  isKnownErrorCode,
  presentErrorCode,
  reportedError,
} from "./presentation";

const M12_CODES = [
  "PROVIDER_NOT_FOUND",
  "PROVIDER_DISABLED",
  "PROVIDER_SLUG_TAKEN",
  "PROVIDER_DISCOVERY_FAILED",
  "PROVIDER_VALIDATION_FAILED",
  "PROVIDER_STATE_INVALID",
  "PROVIDER_AUTH_DENIED",
  "PROVIDER_CODE_EXCHANGE_FAILED",
  "PROVIDER_ID_TOKEN_INVALID",
  "PROVIDER_USERINFO_FAILED",
  "PROVIDER_SUBJECT_MISSING",
  "PROVIDER_EMAIL_UNVERIFIED",
  "PROVIDER_AUTO_PROVISION_DISABLED",
  "PROVIDER_IDENTITY_ALREADY_LINKED",
  "PROVIDER_LINK_NOT_FOUND",
  "PROVIDER_HAS_LINKS",
  "AUTH_EXTERNAL_AMBIGUOUS_IDENTITY",
  "AUTH_EXTERNAL_USERNAME_UNAVAILABLE",
  "AUTH_ACCOUNT_INACTIVE",
  "IDENTITY_LINK_LAST_LOGIN_PATH",
  "PASSWORD_LOGIN_DISABLE_UNSAFE",
  "NO_VALIDATED_PROVIDER",
  "AUTH_RECENT_AUTH_REQUIRED",
];

describe("unit_reported_error", () => {
  test.each(M12_CODES)("%s is a known presented code", (code) => {
    expect(isKnownErrorCode(code)).toBe(true);
    expect(ERROR_PRESENTATION[code as keyof typeof ERROR_PRESENTATION].i18nKey).toMatch(
      /^message\./,
    );
  });

  test("a code reported through a redirect is presented by code, with the real request id", () => {
    const presented = presentErrorCode("PROVIDER_STATE_INVALID", "req-1");

    expect(presented.known).toBe(true);
    expect(presented.requestId).toBe("req-1");
    expect(presented.presentation.i18nKey).toBe("message.providerStateInvalid");
  });

  test("an unknown code uses the generic presentation and keeps the raw code for diagnostics", () => {
    const presented = presentErrorCode("A_FUTURE_CODE", null);

    expect(presented.known).toBe(false);
    expect(presented.code).toBe("A_FUTURE_CODE");
    expect(presented.presentation.i18nKey).toBe("message.unexpected");
  });

  test.each([
    ["PROVIDER_AUTH_DENIED", "req-1", { code: "PROVIDER_AUTH_DENIED", requestId: "req-1" }],
    ["PROVIDER_AUTH_DENIED", null, { code: "PROVIDER_AUTH_DENIED", requestId: null }],
    ["PROVIDER_AUTH_DENIED", "has space", { code: "PROVIDER_AUTH_DENIED", requestId: null }],
    ["PROVIDER_AUTH_DENIED", "x".repeat(200), { code: "PROVIDER_AUTH_DENIED", requestId: null }],
    ["lowercase", "req-1", null],
    ["", "req-1", null],
    [null, "req-1", null],
    ["<script>", "req-1", null],
    ["A".repeat(65), "req-1", null],
  ])("reportedError(%s, %s)", (code, requestId, expected) => {
    expect(reportedError(code, requestId)).toEqual(expected);
  });

  test("the alert always shows the request id, even for codes that normally hide it", async () => {
    const i18n = await loadedI18n();
    render(
      <I18nextProvider i18n={i18n}>
        <ReportedErrorAlert reported={{ code: "PROVIDER_STATE_INVALID", requestId: "req-77" }} />
      </I18nextProvider>,
    );

    expect(await screen.findByText(/expired or was already used/)).toBeDefined();
    expect(screen.getByText("Request ID: req-77")).toBeDefined();
  });
});
