import { expectTypeOf, test } from "vitest";
import type { components } from "../api/schema";
import type { ClientErrorCode, ErrorCode, ServerErrorCode } from "./codes";

test("server codes come from the generated schema", () => {
  expectTypeOf<ServerErrorCode>().toEqualTypeOf<components["schemas"]["ErrorCode"]>();
  expectTypeOf<"INTERNAL_ERROR">().toExtend<ServerErrorCode>();
  expectTypeOf<ServerErrorCode>().toExtend<ErrorCode>();
  expectTypeOf<ClientErrorCode>().toExtend<ErrorCode>();
});

test("the server never uses the client-reserved prefix", () => {
  expectTypeOf<Extract<ServerErrorCode, `CLIENT_${string}`>>().toBeNever();
  expectTypeOf<Exclude<ClientErrorCode, `CLIENT_${string}`>>().toBeNever();
});
