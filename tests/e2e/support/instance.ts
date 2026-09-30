import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const COMPOSE_FILE = resolve(import.meta.dirname, "../compose.yml");
const FIXTURE_DATA_OWNER = "10001:10001";

function composeCommand(): string[] {
  try {
    execFileSync("docker", ["compose", "version"], { stdio: "ignore" });
    return ["docker", "compose"];
  } catch {
    return ["docker-compose"];
  }
}

export function compose(...args: string[]): string {
  const [command = "docker-compose", ...prefix] = composeCommand();
  return execFileSync(command, [...prefix, "--file", COMPOSE_FILE, ...args], {
    encoding: "utf8",
  });
}

export async function waitForLive(baseURL: string) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      if ((await fetch(`${baseURL}/health/live`)).ok) {
        return;
      }
    } catch {
      // the container is still starting
    }
    await new Promise((settle) => setTimeout(settle, 100));
  }
  throw new Error("Palmr did not come back after the operator action");
}

export async function withPalmrStopped<T>(
  baseURL: string,
  action: () => T,
): Promise<T> {
  compose("stop", "palmr");
  try {
    return action();
  } finally {
    compose("start", "palmr");
    await waitForLive(baseURL);
  }
}

export function operatorCommand(...args: string[]): string {
  return compose("run", "--rm", "--no-deps", "palmr", ...args);
}

export async function runJobsOnce(baseURL: string, kind: string) {
  await withPalmrStopped(baseURL, () =>
    operatorCommand("jobs", "run-once", "--kind", kind),
  );
}

export async function operatorResetPassword(
  baseURL: string,
  userId: string,
): Promise<string> {
  const output = await withPalmrStopped(baseURL, () =>
    operatorCommand("user", "reset-password", userId),
  );
  const match = /Temporary password: (\S+)/.exec(output);
  if (match?.[1] === undefined) {
    throw new Error("the operator CLI printed no temporary password");
  }
  return match[1];
}

export async function applyInstanceFixture(baseURL: string, ...args: string[]) {
  const fixture = process.env.PALMR_E2E_FIXTURE_BIN;
  if (fixture === undefined || fixture === "") {
    throw new Error(
      "PALMR_E2E_FIXTURE_BIN is not set; run the E2E suite through tests/e2e/run.sh",
    );
  }
  await withPalmrStopped(baseURL, () => {
    const work = mkdtempSync(join(tmpdir(), "palmr-e2e-data-"));
    try {
      compose("cp", "palmr:/data/.", work);
      execFileSync(fixture, args, {
        env: { ...process.env, PALMR_DATA_DIR: work },
        stdio: "pipe",
      });
      compose(
        "run",
        "--rm",
        "--no-deps",
        "data-seed",
        "find /data -mindepth 1 -delete",
      );
      compose("cp", `${work}/.`, "data-seed:/data");
      compose(
        "run",
        "--rm",
        "--no-deps",
        "data-seed",
        `chown -R ${FIXTURE_DATA_OWNER} /data`,
      );
    } finally {
      rmSync(work, { recursive: true, force: true });
    }
  });
}

export function configureSmtpSink(baseURL: string) {
  return applyInstanceFixture(
    baseURL,
    "smtp-sink",
    "--host",
    "smtp-sink",
    "--port",
    "1025",
    "--from-email",
    "palmr@example.test",
  );
}

export function requireTwoFactor(baseURL: string, required: boolean) {
  return applyInstanceFixture(baseURL, "two-factor-required", String(required));
}
