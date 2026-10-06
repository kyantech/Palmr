export const IDP_ORIGIN =
  process.env.PALMR_E2E_IDP_URL ?? "http://127.0.0.1:9100";

export interface IdpIdentity {
  sub: string;
  email: string;
  email_verified: boolean;
  name: string;
  preferred_username: string;
}

export interface IdpAuthorization {
  clientId: string | null;
  prompt: string | null;
  maxAge: string | null;
  challengeMethod: string | null;
  hasNonce: boolean;
  redirectUri: string | null;
}

async function control<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${IDP_ORIGIN}/__control/${path}`, init);
  if (!response.ok) {
    throw new Error(
      `the mock IdP answered ${String(response.status)} for ${path}`,
    );
  }
  return (await response.json()) as T;
}

export async function waitForIdp() {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const ready = await fetch(
      `${IDP_ORIGIN}/.well-known/openid-configuration`,
    ).then(
      (response) => response.ok,
      () => false,
    );
    if (ready) {
      return;
    }
    await new Promise((settle) => setTimeout(settle, 100));
  }
  throw new Error("the mock IdP did not come up");
}

export function setIdentity(identity: Partial<IdpIdentity>) {
  return control<IdpIdentity>("identity", {
    method: "POST",
    body: JSON.stringify(identity),
  });
}

export function denyNextAuthorization() {
  return control<{ denyNext: boolean }>("deny-next", { method: "POST" });
}

export function authorizations() {
  return control<IdpAuthorization[]>("authorizations");
}

export function resetIdp() {
  return control<{ ok: boolean }>("reset", { method: "POST" });
}
