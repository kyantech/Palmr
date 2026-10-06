import {
  createHash,
  generateKeyPairSync,
  randomBytes,
  sign,
} from "node:crypto";
import { createServer } from "node:http";

const PORT = Number(process.env.MOCK_IDP_PORT ?? "9100");
const ISSUER =
  process.env.MOCK_IDP_ISSUER ?? `http://127.0.0.1:${String(PORT)}`;
const CLIENT_ID = process.env.MOCK_IDP_CLIENT_ID ?? "palmr-e2e";
const CLIENT_SECRET =
  process.env.MOCK_IDP_CLIENT_SECRET ?? "mock-client-secret";
const KEY_ID = "mock-idp-key-1";
const CODE_TTL_MS = 60_000;

const { privateKey, publicKey } = generateKeyPairSync("rsa", {
  modulusLength: 2048,
});
const jwk = {
  ...publicKey.export({ format: "jwk" }),
  kid: KEY_ID,
  alg: "RS256",
  use: "sig",
};

const DEFAULT_IDENTITY = {
  sub: "idp-subject-1",
  email: "person@idp.example.test",
  email_verified: true,
  name: "Idp Person",
  preferred_username: "person",
};

let identity = { ...DEFAULT_IDENTITY };
let denyNext = false;
const codes = new Map();
const authorizeLog = [];

const base64url = (input) => Buffer.from(input).toString("base64url");

function signJwt(claims) {
  const header = base64url(
    JSON.stringify({ alg: "RS256", typ: "JWT", kid: KEY_ID }),
  );
  const payload = base64url(JSON.stringify(claims));
  const signature = sign(
    "RSA-SHA256",
    Buffer.from(`${header}.${payload}`),
    privateKey,
  );
  return `${header}.${payload}.${signature.toString("base64url")}`;
}

function atHash(accessToken) {
  const digest = createHash("sha256").update(accessToken).digest();
  return base64url(digest.subarray(0, digest.length / 2));
}

function respond(response, status, body, headers = {}) {
  const payload = typeof body === "string" ? body : JSON.stringify(body);
  response.writeHead(status, {
    "Content-Type":
      typeof body === "string" ? "text/plain" : "application/json",
    "Cache-Control": "no-store",
    ...headers,
  });
  response.end(payload);
}

async function readBody(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

function discovery() {
  return {
    issuer: ISSUER,
    authorization_endpoint: `${ISSUER}/authorize`,
    token_endpoint: `${ISSUER}/token`,
    userinfo_endpoint: `${ISSUER}/userinfo`,
    jwks_uri: `${ISSUER}/jwks`,
    response_types_supported: ["code"],
    subject_types_supported: ["public"],
    id_token_signing_alg_values_supported: ["RS256"],
    token_endpoint_auth_methods_supported: [
      "client_secret_basic",
      "client_secret_post",
    ],
    scopes_supported: ["openid", "email", "profile"],
    code_challenge_methods_supported: ["S256"],
  };
}

function authorize(url, response) {
  const query = url.searchParams;
  const redirectUri = query.get("redirect_uri");
  const state = query.get("state");
  authorizeLog.push({
    clientId: query.get("client_id"),
    prompt: query.get("prompt"),
    maxAge: query.get("max_age"),
    challengeMethod: query.get("code_challenge_method"),
    hasNonce: query.get("nonce") !== null,
    redirectUri,
  });
  if (
    redirectUri === null ||
    state === null ||
    query.get("client_id") !== CLIENT_ID ||
    query.get("response_type") !== "code" ||
    query.get("code_challenge_method") !== "S256" ||
    query.get("code_challenge") === null
  ) {
    respond(response, 400, "invalid authorization request");
    return;
  }
  const target = new URL(redirectUri);
  target.searchParams.set("state", state);
  if (denyNext) {
    denyNext = false;
    target.searchParams.set("error", "access_denied");
    target.searchParams.set(
      "error_description",
      "The mock user denied the request",
    );
  } else {
    const code = randomBytes(24).toString("hex");
    codes.set(code, {
      clientId: CLIENT_ID,
      redirectUri,
      challenge: query.get("code_challenge"),
      nonce: query.get("nonce"),
      identity: { ...identity },
      authTime: Math.floor(Date.now() / 1000),
      issuedAt: Date.now(),
    });
    target.searchParams.set("code", code);
  }
  response.writeHead(302, {
    Location: target.toString(),
    "Cache-Control": "no-store",
  });
  response.end();
}

function clientAuthenticated(request, params) {
  const header = request.headers.authorization;
  if (header?.startsWith("Basic ")) {
    const [id, secret] = Buffer.from(header.slice(6), "base64")
      .toString("utf8")
      .split(":");
    return (
      decodeURIComponent(id ?? "") === CLIENT_ID &&
      decodeURIComponent(secret ?? "") === CLIENT_SECRET
    );
  }
  return (
    params.get("client_id") === CLIENT_ID &&
    params.get("client_secret") === CLIENT_SECRET
  );
}

async function token(request, response) {
  const params = new URLSearchParams(await readBody(request));
  if (!clientAuthenticated(request, params)) {
    respond(response, 401, { error: "invalid_client" });
    return;
  }
  const code = params.get("code") ?? "";
  const grant = codes.get(code);
  codes.delete(code);
  if (
    params.get("grant_type") !== "authorization_code" ||
    grant === undefined ||
    Date.now() - grant.issuedAt > CODE_TTL_MS ||
    params.get("redirect_uri") !== grant.redirectUri
  ) {
    respond(response, 400, { error: "invalid_grant" });
    return;
  }
  const verifier = params.get("code_verifier") ?? "";
  if (
    base64url(createHash("sha256").update(verifier).digest()) !==
    grant.challenge
  ) {
    respond(response, 400, {
      error: "invalid_grant",
      error_description: "pkce mismatch",
    });
    return;
  }
  const accessToken = randomBytes(24).toString("hex");
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    iss: ISSUER,
    aud: CLIENT_ID,
    sub: grant.identity.sub,
    iat: now,
    nbf: now - 5,
    exp: now + 300,
    auth_time: grant.authTime,
    at_hash: atHash(accessToken),
    email: grant.identity.email,
    email_verified: grant.identity.email_verified,
    name: grant.identity.name,
    preferred_username: grant.identity.preferred_username,
    ...(grant.nonce === null ? {} : { nonce: grant.nonce }),
  };
  accessTokens.set(accessToken, grant.identity);
  respond(response, 200, {
    access_token: accessToken,
    token_type: "Bearer",
    expires_in: 300,
    scope: "openid email profile",
    id_token: signJwt(claims),
  });
}

const accessTokens = new Map();

function userinfo(request, response) {
  const bearer = request.headers.authorization?.replace(/^Bearer /, "") ?? "";
  const found = accessTokens.get(bearer);
  if (found === undefined) {
    respond(response, 401, { error: "invalid_token" });
    return;
  }
  respond(response, 200, found);
}

async function control(request, url, response) {
  if (url.pathname === "/__control/identity" && request.method === "POST") {
    identity = { ...DEFAULT_IDENTITY, ...JSON.parse(await readBody(request)) };
    respond(response, 200, identity);
  } else if (
    url.pathname === "/__control/deny-next" &&
    request.method === "POST"
  ) {
    denyNext = true;
    respond(response, 200, { denyNext });
  } else if (
    url.pathname === "/__control/authorizations" &&
    request.method === "GET"
  ) {
    respond(response, 200, authorizeLog);
  } else if (url.pathname === "/__control/reset" && request.method === "POST") {
    identity = { ...DEFAULT_IDENTITY };
    denyNext = false;
    authorizeLog.length = 0;
    respond(response, 200, { ok: true });
  } else {
    respond(response, 404, "not found");
  }
}

createServer((request, response) => {
  const url = new URL(request.url ?? "/", ISSUER);
  const handled = (async () => {
    if (url.pathname === "/.well-known/openid-configuration") {
      respond(response, 200, discovery());
    } else if (url.pathname === "/jwks") {
      respond(response, 200, { keys: [jwk] });
    } else if (url.pathname === "/authorize") {
      authorize(url, response);
    } else if (url.pathname === "/token" && request.method === "POST") {
      await token(request, response);
    } else if (url.pathname === "/userinfo") {
      userinfo(request, response);
    } else if (url.pathname.startsWith("/__control/")) {
      await control(request, url, response);
    } else {
      respond(response, 404, "not found");
    }
  })();
  handled.catch((error) => {
    console.error(error);
    respond(response, 500, "mock idp failure");
  });
}).listen(PORT, "0.0.0.0", () => {
  console.log(`mock idp listening on ${String(PORT)} as ${ISSUER}`);
});
