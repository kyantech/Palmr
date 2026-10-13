import { createHash, randomBytes } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import http from "node:http";
import { join } from "node:path";
import { Readable } from "node:stream";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import * as tus from "tus-js-client";
import {
  cookieHeader,
  Palmr,
  type Principal,
  type RawResponse,
} from "../palmr.ts";

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

const OCTET = "application/offset+octet-stream";

function sha256(data: Buffer): string {
  return createHash("sha256").update(data).digest("hex");
}

function checksumHeader(data: Buffer): string {
  return `sha256 ${createHash("sha256").update(data).digest("base64")}`;
}

function uploadId(url: string | null): string {
  const id = (url ?? "").split("/").pop();
  if (id === undefined || id === "") {
    throw new Error("the upload has no URL");
  }
  return id;
}

function stagedBlob(url: string | null): Buffer {
  return readFileSync(
    join(palmr.dataDir, "uploads", uploadId(url).replaceAll("-", ""), "blob"),
  );
}

function stagedLength(url: string | null): number {
  return statSync(
    join(palmr.dataDir, "uploads", uploadId(url).replaceAll("-", ""), "blob"),
  ).size;
}

async function headOffset(principal: Principal, url: string | null) {
  const response = await palmr.raw("HEAD", new URL(url ?? "").pathname, {
    headers: authenticated(principal),
  });
  expect(response.status).toBe(200);
  return Number(response.headers["upload-offset"]);
}

async function waitFor(
  condition: () => Promise<boolean>,
  what: string,
  timeoutMs = 15_000,
) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await condition()) {
      return;
    }
    await new Promise((settle) => setTimeout(settle, 50));
  }
  throw new Error(`timed out waiting for ${what}`);
}

async function creation(
  principal: Principal,
  item: { sessionId: string; itemId: string; name: string },
  length: number | null,
) {
  const response = await palmr.raw("POST", TUS_PATH, {
    headers: authenticated(principal, {
      ...(length === null
        ? { "upload-defer-length": "1" }
        : { "upload-length": String(length) }),
      "upload-metadata": [
        `filename ${Buffer.from(item.name).toString("base64")}`,
        `transferSessionId ${Buffer.from(item.sessionId).toString("base64")}`,
        `itemId ${Buffer.from(item.itemId).toString("base64")}`,
      ].join(","),
    }),
  });
  expect(response.status).toBe(201);
  const url = response.headers.location ?? "";
  return { url, path: new URL(url).pathname };
}

function openPatch(
  principal: Principal,
  path: string,
  offset: number,
  extra: Record<string, string> = {},
) {
  const request = http.request(new URL(path, palmr.baseUrl), {
    method: "PATCH",
    headers: {
      ...authenticated(principal),
      "content-type": OCTET,
      "upload-offset": String(offset),
      "transfer-encoding": "chunked",
      ...extra,
    },
  });
  const done = new Promise<RawResponse>((settle, reject) => {
    request.on("response", (response) => {
      const chunks: Buffer[] = [];
      response.on("data", (chunk: Buffer) => chunks.push(chunk));
      response.on("end", () =>
        settle({
          status: response.statusCode ?? 0,
          headers: response.headers,
          body: Buffer.concat(chunks),
        }),
      );
    });
    request.on("error", reject);
  });
  return { request, done };
}

function patchOnce(
  principal: Principal,
  path: string,
  offset: number,
  body: Buffer,
  extra: Record<string, string> = {},
) {
  return palmr.raw("PATCH", path, {
    headers: {
      ...authenticated(principal),
      "content-type": OCTET,
      "upload-offset": String(offset),
      "content-length": String(body.length),
      ...extra,
    },
    body,
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

  it("appends a body in several PATCH requests and resumes from HEAD (tus-js-client)", async () => {
    const payload = randomBytes(950_000);
    const item = await planned(alice, "chunked.bin", payload.length);
    let patches = 0;
    const offsets: string[] = [];
    const client = await upload(alice, payload, item, {
      uploadDataDuringCreation: false,
      chunkSize: 300_000,
      onBeforeRequest: (request) => {
        if (request.getMethod() === "PATCH") {
          patches += 1;
          offsets.push(request.getHeader("Upload-Offset") ?? "");
        }
      },
    });
    expect(patches).toBe(4);
    expect(offsets).toEqual(["0", "300000", "600000", "900000"]);
    expect(await headOffset(alice, client.url)).toBe(payload.length);
    expect(sha256(stagedBlob(client.url))).toBe(sha256(payload));
  });

  it("streams one long PATCH and reports the protocol headers", async () => {
    const payload = randomBytes(6_000_000);
    const item = await planned(alice, "long.bin", payload.length);
    const client = await upload(alice, payload, item, {
      uploadDataDuringCreation: false,
    });
    expect(sha256(stagedBlob(client.url))).toBe(sha256(payload));

    const probe = await planned(alice, "headers.bin", 10);
    const { path } = await creation(alice, probe, 10);
    const response = await patchOnce(alice, path, 0, Buffer.alloc(10, 7));
    expect(response.status).toBe(204);
    expect(response.body.length).toBe(0);
    expect(response.headers["tus-resumable"]).toBe("1.0.0");
    expect(response.headers["upload-offset"]).toBe("10");
    expect(response.headers["upload-expires"]).toMatch(/ GMT$/);
    expect(response.headers["cache-control"]).toBe("no-store");
    expect(response.headers["x-request-id"]).toBeDefined();
    expect(response.headers["content-type"]).toBeUndefined();
  });

  it("uploads a streamed body of deferred length through chunked PATCH requests", async () => {
    const payload = randomBytes(700_000);
    const item = await planned(alice, "deferred.bin", null);
    const client = await new Promise<tus.Upload>((resolve, reject) => {
      const stream = Readable.from([payload]);
      const started = new tus.Upload(stream, {
        endpoint: `${palmr.baseUrl}${TUS_PATH}`,
        retryDelays: null,
        uploadLengthDeferred: true,
        chunkSize: 250_000,
        metadata: {
          filename: item.name,
          transferSessionId: item.sessionId,
          itemId: item.itemId,
        },
        headers: {
          cookie: cookieHeader(alice),
          origin: palmr.baseUrl,
          "x-palmr-csrf": alice.csrf,
        },
        onError: reject,
        onSuccess: () => resolve(started),
      });
      started.start();
    });
    const head = await palmr.raw("HEAD", new URL(client.url ?? "").pathname, {
      headers: authenticated(alice),
    });
    expect(head.headers["upload-length"]).toBe(String(payload.length));
    expect(head.headers["upload-defer-length"]).toBeUndefined();
    expect(sha256(stagedBlob(client.url))).toBe(sha256(payload));
  });

  it("verifies Upload-Checksum per PATCH body", async () => {
    const payload = randomBytes(450_000);
    const item = await planned(alice, "checksummed.bin", payload.length);
    const client = await upload(alice, payload, item, {
      uploadDataDuringCreation: false,
      chunkSize: 200_000,
      onBeforeRequest: (request) => {
        if (request.getMethod() === "PATCH") {
          const offset = Number(request.getHeader("Upload-Offset") ?? "0");
          request.setHeader(
            "Upload-Checksum",
            checksumHeader(payload.subarray(offset, offset + 200_000)),
          );
        }
      },
    });
    expect(sha256(stagedBlob(client.url))).toBe(sha256(payload));

    const other = await planned(alice, "mismatch.bin", 100_000);
    const { path, url } = await creation(alice, other, 100_000);
    const body = randomBytes(100_000);
    const wrong = await patchOnce(alice, path, 0, body, {
      "upload-checksum": checksumHeader(Buffer.from("something else")),
    });
    expect(wrong.status).toBe(460);
    expect(JSON.parse(wrong.body.toString()).error.code).toBe(
      "UPLOAD_CHECKSUM_MISMATCH",
    );
    expect(await headOffset(alice, url)).toBe(0);
    expect(stagedLength(url)).toBe(0);
    const retried = await patchOnce(alice, path, 0, body, {
      "upload-checksum": checksumHeader(body),
    });
    expect(retried.status).toBe(204);
    expect(retried.headers["upload-offset"]).toBe("100000");

    for (const bad of ["sha256", "md5 abc", "sha256 !!!"]) {
      const response = await patchOnce(alice, path, 100_000, Buffer.alloc(0), {
        "upload-checksum": bad,
      });
      expect(response.status).toBe(400);
    }
  });

  it("answers a stale Upload-Offset with 409 and the true offset", async () => {
    const item = await planned(alice, "stale.bin", 50_000);
    const { path } = await creation(alice, item, 50_000);
    const first = await patchOnce(alice, path, 0, randomBytes(20_000));
    expect(first.status).toBe(204);
    for (const stale of [0, 5_000, 40_000]) {
      const response = await patchOnce(alice, path, stale, randomBytes(100));
      expect(response.status).toBe(409);
      expect(response.headers["upload-offset"]).toBe("20000");
      expect(response.headers["tus-resumable"]).toBe("1.0.0");
      const error = JSON.parse(response.body.toString()).error;
      expect(error.code).toBe("UPLOAD_OFFSET_MISMATCH");
      expect(error.details.expectedOffset).toBe(20_000);
    }
  });

  it("validates the PATCH headers before accepting a byte", async () => {
    const item = await planned(alice, "headers-check.bin", 1_000);
    const { path, url } = await creation(alice, item, 1_000);
    const body = randomBytes(100);
    const version = await palmr.raw("PATCH", path, {
      headers: {
        ...authenticated(alice),
        "tus-resumable": "0.2.2",
        "content-type": OCTET,
        "upload-offset": "0",
      },
      body,
    });
    expect(version.status).toBe(412);
    expect(version.headers["tus-version"]).toBe("1.0.0");
    const media = await palmr.raw("PATCH", path, {
      headers: {
        ...authenticated(alice),
        "content-type": "application/octet-stream",
        "upload-offset": "0",
      },
      body,
    });
    expect(media.status).toBe(415);
    const missing = await palmr.raw("PATCH", path, {
      headers: { ...authenticated(alice), "content-type": OCTET },
      body,
    });
    expect(missing.status).toBe(400);
    const csrf = await palmr.raw("PATCH", path, {
      headers: {
        ...authenticated(alice),
        "x-palmr-csrf": "not-the-token",
        "content-type": OCTET,
        "upload-offset": "0",
      },
      body,
    });
    expect(csrf.status).toBe(403);
    expect(stagedLength(url)).toBe(0);
    const foreign = await patchOnce(bob, path, 0, body);
    expect(foreign.status).toBe(404);
    expect(stagedLength(url)).toBe(0);
  });

  it("refuses a second writer with 423 while the first holds the upload", async () => {
    const item = await planned(alice, "locked.bin", 400_000);
    const { path, url } = await creation(alice, item, 400_000);
    const payload = randomBytes(400_000);
    const first = openPatch(alice, path, 0);
    first.request.write(payload.subarray(0, 100_000));
    await waitFor(
      async () => (await headOffset(alice, url)) > 0,
      "the first writer's checkpoint",
    );
    const second = await patchOnce(alice, path, 0, payload.subarray(0, 100));
    expect(second.status).toBe(423);
    expect(JSON.parse(second.body.toString()).error.code).toBe("UPLOAD_LOCKED");
    const held = await headOffset(alice, url);
    first.request.end(payload.subarray(100_000));
    const finished = await first.done;
    expect(finished.status).toBe(204);
    expect(held).toBeGreaterThan(0);
    expect(sha256(stagedBlob(url))).toBe(sha256(payload));
    expect(await headOffset(alice, url)).toBe(payload.length);
  });

  it("resumes an interrupted PATCH from the authoritative offset", async () => {
    const payload = randomBytes(800_000);
    const item = await planned(alice, "interrupted.bin", payload.length);
    const { url, path } = await creation(alice, item, payload.length);
    const doomed = openPatch(alice, path, 0);
    doomed.done.catch(() => undefined);
    doomed.request.write(payload.subarray(0, 300_000));
    await waitFor(
      async () => (await headOffset(alice, url)) > 0,
      "a persisted offset",
    );
    doomed.request.destroy();
    let settled = -1;
    await waitFor(async () => {
      const current = await headOffset(alice, url);
      const stable = current === settled;
      settled = current;
      return stable && current > 0;
    }, "the interrupted request to settle");

    const offset = await headOffset(alice, url);
    expect(offset).toBeGreaterThan(0);
    expect(offset).toBeLessThanOrEqual(300_000);
    expect(stagedLength(url)).toBe(offset);
    const resumed = await upload(alice, payload, item, {
      uploadUrl: url,
      uploadDataDuringCreation: false,
      chunkSize: 250_000,
    });
    expect(resumed.url).toBe(url);
    expect(await headOffset(alice, url)).toBe(payload.length);
    expect(sha256(stagedBlob(url))).toBe(sha256(payload));
  });

  it("rejects bytes beyond the declared length with 413", async () => {
    const item = await planned(alice, "overrun.bin", 1_000);
    const { path } = await creation(alice, item, 1_000);
    const response = await patchOnce(alice, path, 0, randomBytes(1_001));
    expect(response.status).toBe(413);
    expect(JSON.parse(response.body.toString()).error.code).toBe(
      "FILE_TOO_LARGE",
    );
  });

  it("accepts X-HTTP-Method-Override: PATCH on the resource POST", async () => {
    const item = await planned(alice, "override.bin", 2_000);
    const { path, url } = await creation(alice, item, 2_000);
    const body = randomBytes(2_000);
    const response = await palmr.raw("POST", path, {
      headers: {
        ...authenticated(alice),
        "x-http-method-override": "PATCH",
        "content-type": OCTET,
        "upload-offset": "0",
      },
      body,
    });
    expect(response.status).toBe(204);
    expect(response.headers["upload-offset"]).toBe("2000");
    expect(sha256(stagedBlob(url))).toBe(sha256(body));
  });
});
