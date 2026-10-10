import { randomBytes } from "node:crypto";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import * as tus from "tus-js-client";
import { cookieHeader, Palmr, type Principal } from "../palmr.ts";

const TUS_PATH = "/api/v1/uploads/tus";

let palmr: Palmr;
let alice: Principal;
let bob: Principal;
let nextClientId = 0;

beforeAll(async () => {
  palmr = await Palmr.start();
  const admin = await palmr.bootstrapAdmin();
  alice = admin;
  bob = await palmr.createUser(admin, "bob");
}, 60_000);

afterAll(async () => {
  await palmr.stop();
});

function authenticated(
  principal: Principal,
  extra: Record<string, string> = {},
) {
  return {
    cookie: cookieHeader(principal),
    origin: palmr.baseUrl,
    "x-palmr-csrf": principal.csrf,
    "tus-resumable": "1.0.0",
    ...extra,
  };
}

async function planned(
  principal: Principal,
  name: string,
  size: number | null,
) {
  nextClientId += 1;
  const { sessionId, itemIds } = await palmr.plan(principal, [
    { clientId: `conformance-${nextClientId}`, name, sizeBytes: size },
  ]);
  const itemId = itemIds[0];
  if (itemId === undefined) {
    throw new Error("the session has no item");
  }
  return { sessionId, itemId, name };
}

function upload(
  principal: Principal,
  body: Buffer,
  item: { sessionId: string; itemId: string; name: string },
  options: Partial<tus.UploadOptions> = {},
) {
  return new Promise<tus.Upload>((resolve, reject) => {
    const client = new tus.Upload(body, {
      endpoint: `${palmr.baseUrl}${TUS_PATH}`,
      retryDelays: null,
      uploadDataDuringCreation: true,
      metadata: {
        filename: item.name,
        filetype: "application/octet-stream",
        transferSessionId: item.sessionId,
        itemId: item.itemId,
      },
      headers: {
        cookie: cookieHeader(principal),
        origin: palmr.baseUrl,
        "x-palmr-csrf": principal.csrf,
      },
      onError: reject,
      onSuccess: () => resolve(client),
      ...options,
    });
    client.start();
  });
}

describe("TUS 1.0 supported surface against a real Palmr process", () => {
  it("advertises exactly the supported extensions on OPTIONS", async () => {
    const response = await palmr.raw("OPTIONS", TUS_PATH, {
      headers: { cookie: cookieHeader(alice) },
    });
    expect(response.status).toBe(204);
    expect(response.body.length).toBe(0);
    expect(response.headers["tus-resumable"]).toBe("1.0.0");
    expect(response.headers["tus-version"]).toBe("1.0.0");
    expect(response.headers["tus-extension"]).toBe(
      "creation,creation-with-upload,expiration,termination,checksum",
    );
    expect(response.headers["tus-checksum-algorithm"]).toBe("sha256");
    expect(response.headers["tus-max-size"]).toBeUndefined();
  });

  it("creates a resource and carries the initial bytes (creation-with-upload)", async () => {
    const payload = randomBytes(300_000);
    const item = await planned(alice, "conformance.bin", payload.length);
    const client = await upload(alice, payload, item);

    const location = client.url;
    expect(location).toMatch(
      new RegExp(`^${palmr.baseUrl}${TUS_PATH}/[0-9a-f-]{36}$`),
    );
    const head = await palmr.raw("HEAD", new URL(location ?? "").pathname, {
      headers: authenticated(alice),
    });
    expect(head.status).toBe(200);
    expect(head.headers["upload-offset"]).toBe(String(payload.length));
    expect(head.headers["upload-length"]).toBe(String(payload.length));
    expect(head.headers["cache-control"]).toBe("no-store");
    expect(head.headers["upload-expires"]).toMatch(
      /^[A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} GMT$/,
    );
    expect(head.body.length).toBe(0);
  });

  it("lets the client resume from the authoritative offset (HEAD)", async () => {
    const payload = randomBytes(120_000);
    const item = await planned(alice, "resume.bin", payload.length);
    const first = await upload(alice, payload, item);
    const url = first.url;
    expect(url).not.toBeNull();

    const resumed = await upload(alice, payload, item, {
      uploadUrl: url,
      uploadDataDuringCreation: false,
    });
    expect(resumed.url).toBe(url);
  });

  it("answers a repeated creation with the same resource", async () => {
    const payload = randomBytes(2_000);
    const item = await planned(alice, "twice.bin", payload.length);
    const first = await upload(alice, payload, item);
    const again = await palmr.raw("POST", TUS_PATH, {
      headers: authenticated(alice, {
        "upload-length": String(payload.length),
        "upload-metadata": [
          `filename ${Buffer.from(item.name).toString("base64")}`,
          `transferSessionId ${Buffer.from(item.sessionId).toString("base64")}`,
          `itemId ${Buffer.from(item.itemId).toString("base64")}`,
        ].join(","),
      }),
    });
    expect(again.status).toBe(201);
    expect(again.headers.location).toBe(first.url);
    expect(again.headers["upload-offset"]).toBe(String(payload.length));
  });

  it("terminates an upload and never resumes it", async () => {
    const item = await planned(alice, "terminated.bin", 5_000);
    const created = await palmr.raw("POST", TUS_PATH, {
      headers: authenticated(alice, {
        "upload-length": "5000",
        "upload-metadata": [
          `filename ${Buffer.from(item.name).toString("base64")}`,
          `transferSessionId ${Buffer.from(item.sessionId).toString("base64")}`,
          `itemId ${Buffer.from(item.itemId).toString("base64")}`,
        ].join(","),
      }),
    });
    expect(created.status).toBe(201);
    const url = created.headers.location ?? "";
    await tus.Upload.terminate(url, {
      headers: {
        cookie: cookieHeader(alice),
        origin: palmr.baseUrl,
        "x-palmr-csrf": alice.csrf,
      },
    });
    const head = await palmr.raw("HEAD", new URL(url).pathname, {
      headers: authenticated(alice),
    });
    expect(head.status).toBe(410);
    const again = await palmr.raw("DELETE", new URL(url).pathname, {
      headers: authenticated(alice),
    });
    expect(again.status).toBe(204);
  });

  it("negotiates the protocol version", async () => {
    for (const version of [undefined, "1.0.1", "0.2.2"]) {
      const headers = authenticated(
        alice,
        version === undefined ? {} : { "tus-resumable": version },
      );
      if (version === undefined) {
        delete (headers as Record<string, string | undefined>)["tus-resumable"];
      }
      const response = await palmr.raw("POST", TUS_PATH, { headers });
      expect(response.status).toBe(412);
      expect(response.headers["tus-version"]).toBe("1.0.0");
      expect(JSON.parse(response.body.toString()).error.code).toBe(
        "TUS_VERSION_UNSUPPORTED",
      );
    }
  });

  it("rejects concatenation with 501", async () => {
    const item = await planned(alice, "concat.bin", 10);
    const response = await palmr.raw("POST", TUS_PATH, {
      headers: authenticated(alice, {
        "upload-concat": "partial",
        "upload-length": "10",
        "upload-metadata": [
          `filename ${Buffer.from(item.name).toString("base64")}`,
          `transferSessionId ${Buffer.from(item.sessionId).toString("base64")}`,
          `itemId ${Buffer.from(item.itemId).toString("base64")}`,
        ].join(","),
      }),
    });
    expect(response.status).toBe(501);
    expect(JSON.parse(response.body.toString()).error.code).toBe(
      "TUS_EXTENSION_UNSUPPORTED",
    );
  });

  it("hides another owner's upload behind 404", async () => {
    const payload = randomBytes(1_000);
    const item = await planned(alice, "private.bin", payload.length);
    const client = await upload(alice, payload, item);
    const path = new URL(client.url ?? "").pathname;
    for (const method of ["HEAD", "DELETE"]) {
      const response = await palmr.raw(method, path, {
        headers: authenticated(bob),
      });
      expect(response.status).toBe(404);
      expect(response.headers["tus-resumable"]).toBe("1.0.0");
    }
    const stillAlices = await palmr.raw("HEAD", path, {
      headers: authenticated(alice),
    });
    expect(stillAlices.status).toBe(200);
  });

  it("builds Location only from the configured base URL", async () => {
    const payload = randomBytes(10);
    const item = await planned(alice, "host.bin", payload.length);
    const response = await palmr.raw("POST", TUS_PATH, {
      headers: authenticated(alice, {
        host: "evil.example",
        "x-forwarded-host": "evil.example",
        "x-forwarded-proto": "https",
        "upload-length": "10",
        "upload-metadata": [
          `filename ${Buffer.from(item.name).toString("base64")}`,
          `transferSessionId ${Buffer.from(item.sessionId).toString("base64")}`,
          `itemId ${Buffer.from(item.itemId).toString("base64")}`,
        ].join(","),
      }),
    });
    expect(response.status).toBe(201);
    expect(response.headers.location).toMatch(
      new RegExp(`^${palmr.baseUrl}${TUS_PATH}/`),
    );
  });

  it.todo(
    "full TUS 1.0 upload conformance (PATCH, offset mismatch, locking, checksum) is pending M14-T04",
  );
});
