import { type ChildProcess, spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import http from "node:http";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const PASSWORD = "correct horse battery staple";

export interface RawResponse {
  status: number;
  headers: http.IncomingHttpHeaders;
  body: Buffer;
}

export interface Principal {
  username: string;
  cookies: Map<string, string>;
  csrf: string;
}

async function freePort(): Promise<number> {
  return await new Promise((resolvePort, reject) => {
    const probe = createServer();
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      probe.close(() => {
        if (address !== null && typeof address === "object") {
          resolvePort(address.port);
        } else {
          reject(new Error("no port was assigned"));
        }
      });
    });
  });
}

export function cookieHeader(principal: Principal): string {
  return [...principal.cookies]
    .map(([name, value]) => `${name}=${value}`)
    .join("; ");
}

export class Palmr {
  readonly baseUrl: string;
  private readonly child: ChildProcess;
  private readonly dataDir: string;
  private log = "";

  private constructor(baseUrl: string, child: ChildProcess, dataDir: string) {
    this.baseUrl = baseUrl;
    this.child = child;
    this.dataDir = dataDir;
    child.stdout?.on("data", (chunk: Buffer) => this.capture(chunk));
    child.stderr?.on("data", (chunk: Buffer) => this.capture(chunk));
  }

  private capture(chunk: Buffer) {
    this.log = (this.log + chunk.toString("utf8")).slice(-20_000);
  }

  static async start(): Promise<Palmr> {
    const binary =
      process.env.PALMR_BIN ??
      resolve(import.meta.dirname, "../../target/debug/palmr");
    const dataDir = mkdtempSync(join(tmpdir(), "palmr-tus-conformance-"));
    const port = await freePort();
    const baseUrl = `http://127.0.0.1:${port}`;
    const child = spawn(binary, [], {
      env: {
        PATH: process.env.PATH ?? "",
        PALMR_DATA_DIR: dataDir,
        PALMR_HOST: "127.0.0.1",
        PALMR_PORT: String(port),
        PALMR_BASE_URL: baseUrl,
        PALMR_LOG_LEVEL: "warn",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const palmr = new Palmr(baseUrl, child, dataDir);
    await palmr.waitForLive();
    return palmr;
  }

  private async waitForLive() {
    for (let attempt = 0; attempt < 300; attempt += 1) {
      if (this.child.exitCode !== null) {
        throw new Error(
          `palmr exited early (${this.child.exitCode}): ${this.log}`,
        );
      }
      try {
        if ((await this.raw("GET", "/health/live")).status === 200) {
          return;
        }
      } catch {
        // the listener is not up yet
      }
      await new Promise((settle) => setTimeout(settle, 100));
    }
    throw new Error(`palmr did not become live: ${this.log}`);
  }

  async stop() {
    this.child.kill("SIGTERM");
    await new Promise((settle) => {
      if (this.child.exitCode !== null) {
        settle(undefined);
        return;
      }
      this.child.once("exit", () => settle(undefined));
      setTimeout(() => {
        this.child.kill("SIGKILL");
        settle(undefined);
      }, 10_000);
    });
    rmSync(this.dataDir, { recursive: true, force: true });
  }

  async raw(
    method: string,
    path: string,
    options: {
      headers?: Record<string, string>;
      body?: Buffer | string;
    } = {},
  ): Promise<RawResponse> {
    const target = new URL(path, this.baseUrl);
    return await new Promise((settle, reject) => {
      const request = http.request(
        target,
        { method, headers: options.headers },
        (response) => {
          const chunks: Buffer[] = [];
          response.on("data", (chunk: Buffer) => chunks.push(chunk));
          response.on("end", () =>
            settle({
              status: response.statusCode ?? 0,
              headers: response.headers,
              body: Buffer.concat(chunks),
            }),
          );
        },
      );
      request.on("error", reject);
      request.end(options.body);
    });
  }

  private remember(principal: Principal, response: RawResponse) {
    for (const entry of response.headers["set-cookie"] ?? []) {
      const [pair] = entry.split(";");
      const [name, ...value] = (pair ?? "").split("=");
      if (name !== undefined && name !== "") {
        principal.cookies.set(name, value.join("="));
      }
    }
    const csrf = principal.cookies.get("palmr_csrf");
    if (csrf !== undefined) {
      principal.csrf = csrf;
    }
  }

  private async json(
    principal: Principal,
    method: string,
    path: string,
    body: unknown,
  ): Promise<RawResponse> {
    const response = await this.raw(method, path, {
      headers: {
        "content-type": "application/json",
        cookie: cookieHeader(principal),
        origin: this.baseUrl,
        "x-palmr-csrf": principal.csrf,
      },
      body: JSON.stringify(body),
    });
    this.remember(principal, response);
    return response;
  }

  async bootstrapAdmin(): Promise<Principal> {
    const principal: Principal = {
      username: "admin",
      cookies: new Map(),
      csrf: "",
    };
    this.remember(principal, await this.raw("GET", "/api/v1/setup/status"));
    const created = await this.json(principal, "POST", "/api/v1/setup", {
      appName: "Palmr",
      firstName: "Ada",
      lastName: "Admin",
      username: "admin",
      email: "admin@example.test",
      password: PASSWORD,
      locale: "en-US",
    });
    if (created.status !== 201) {
      throw new Error(
        `setup failed: ${created.status} ${created.body.toString()}`,
      );
    }
    return principal;
  }

  async login(username: string, password: string): Promise<Principal> {
    const principal: Principal = { username, cookies: new Map(), csrf: "" };
    this.remember(principal, await this.raw("GET", "/api/v1/setup/status"));
    const response = await this.json(principal, "POST", "/api/v1/auth/login", {
      identifier: username,
      password,
    });
    if (response.status !== 200) {
      throw new Error(
        `login failed: ${response.status} ${response.body.toString()}`,
      );
    }
    return principal;
  }

  async api(
    principal: Principal,
    method: string,
    path: string,
    body?: unknown,
  ) {
    return await this.json(principal, method, path, body ?? {});
  }

  async plan(
    principal: Principal,
    files: {
      clientId: string;
      name: string;
      sizeBytes: number | null;
      relativePath?: string;
    }[],
  ): Promise<{ sessionId: string; itemIds: string[] }> {
    const response = await this.api(
      principal,
      "POST",
      "/api/v1/transfers/sessions",
      {
        target: { kind: "my_files" },
        files,
      },
    );
    if (response.status !== 201) {
      throw new Error(
        `session failed: ${response.status} ${response.body.toString()}`,
      );
    }
    const session = JSON.parse(response.body.toString()) as {
      id: string;
      files: { itemId: string }[];
    };
    return {
      sessionId: session.id,
      itemIds: session.files.map((file) => file.itemId),
    };
  }

  async createUser(admin: Principal, username: string): Promise<Principal> {
    const response = await this.api(admin, "POST", "/api/v1/admin/users", {
      username,
      email: `${username}@example.test`,
      firstName: "Bob",
      lastName: "User",
      password: PASSWORD,
      role: "user",
      requirePasswordChange: false,
    });
    if (response.status !== 201) {
      throw new Error(
        `user failed: ${response.status} ${response.body.toString()}`,
      );
    }
    return await this.login(username, PASSWORD);
  }
}

export { PASSWORD };
