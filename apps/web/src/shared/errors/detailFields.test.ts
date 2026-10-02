import { expect, test } from "vitest";
import { ApiError, detailFields, type ErrorDetails } from ".";

function error(details: ErrorDetails): ApiError {
  return new ApiError({
    code: "VALIDATION_ERROR",
    status: 422,
    requestId: "request-1",
    details,
    request: { method: "POST", path: "/api/v1/example" },
    serverMessage: "The request failed validation",
  });
}

test("detailFields returns the invalid field names only", () => {
  expect(detailFields(error({ fields: ["slug", "clientSecret"] }))).toEqual([
    "slug",
    "clientSecret",
  ]);
  expect(detailFields(error({}))).toEqual([]);
  expect(detailFields(error({ fields: "slug" }))).toEqual([]);
  expect(
    detailFields(
      error({
        fields: [
          { name: "jwks", ok: false, detail: "unreachable" },
          "issuerUrl",
        ] as unknown as string[],
      }),
    ),
  ).toEqual(["issuerUrl"]);
});

test("a provider validation error carries its failing checks without widening the field list", () => {
  const failed = new ApiError({
    code: "PROVIDER_VALIDATION_FAILED",
    status: 422,
    requestId: "request-2",
    details: { checks: [{ name: "jwks", ok: false, detail: "unreachable" }] },
    request: { method: "POST", path: "/api/v1/admin/providers/id/test" },
    serverMessage: "The identity provider checks failed",
  });
  expect(failed.details.checks).toEqual([{ name: "jwks", ok: false, detail: "unreachable" }]);
});
