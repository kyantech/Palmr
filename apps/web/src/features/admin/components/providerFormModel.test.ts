import { describe, expect, test } from "vitest";
import { defaultProviders, presetCatalogue } from "../../../test/providerServer";
import {
  applyDiscovery,
  createBody,
  emptyValues,
  parseValidationError,
  presetKeyOf,
  updateBody,
  valuesFromPreset,
  valuesFromProvider,
} from "./providerFormModel";
import { must } from "../../../test/must";

const [google, authentik, github] = defaultProviders();
const presets = presetCatalogue();

describe("unit_provider_form_model", () => {
  test("a preset initialises everything from the server catalogue and never enables or auto-provisions", () => {
    const values = valuesFromPreset(must(presets[1]));

    expect(values).toMatchObject({
      presetKey: "github:oauth2",
      slug: "github",
      displayName: "GitHub",
      protocol: "oauth2",
      preset: "github",
      autoProvision: false,
      enabled: false,
      allowEmailLinking: false,
      clientSecret: "",
      authorizationEndpoint: "https://github.com/login/oauth/authorize",
    });
  });

  test("custom presets share the generic key and are told apart by protocol", () => {
    const keys = presets.map(presetKeyOf);

    expect(keys).toContain("generic:oidc");
    expect(keys).toContain("generic:oauth2");
    expect(new Set(keys).size).toBe(keys.length);
    expect(valuesFromPreset(must(presets[2])).slug).toBe("");
  });

  test("editing starts with an empty secret", () => {
    const values = valuesFromProvider(must(google));

    expect(values.clientSecret).toBe("");
    expect(values.clearSecret).toBe(false);
  });

  test("a create body omits the secret when empty and never carries slug-independent server fields", () => {
    const body = createBody({
      ...valuesFromPreset(must(presets[0])),
      clientId: "id",
      clientSecret: "",
      tokenAuthMethod: "none",
    });

    expect(body).not.toHaveProperty("clientSecret");
    expect(body).not.toHaveProperty("redirectUri");
    expect(body).not.toHaveProperty("role");
    expect(body).toMatchObject({ protocol: "oidc", preset: "google", tokenAuthMethod: "none" });
  });

  test("an update with no edits is empty", () => {
    expect(updateBody(must(google), valuesFromProvider(must(google)))).toEqual({});
  });

  test("an update sends only what changed, and endpoint members are per-member", () => {
    const values = {
      ...valuesFromProvider(must(authentik)),
      displayName: "Renamed",
      tokenEndpoint: "https://sso.example.test/other-token",
      jwksEndpoint: "",
      claimEmail: "mail",
    };

    expect(updateBody(must(authentik), values)).toEqual({
      displayName: "Renamed",
      endpoints: { token: "https://sso.example.test/other-token", jwks: null },
      claimMapping: { email: "mail" },
    });
  });

  test("clearing the secret needs the none method and a stored secret", () => {
    const base = valuesFromProvider(must(google));

    expect(updateBody(must(google), { ...base, clearSecret: true })).toEqual({});
    expect(
      updateBody(must(google), { ...base, clearSecret: true, tokenAuthMethod: "none" }),
    ).toEqual({
      tokenAuthMethod: "none",
      clientSecret: null,
    });
    expect(updateBody(must(google), { ...base, clientSecret: "typed" })).toEqual({
      clientSecret: "typed",
    });
  });

  test("an update never names the slug or the protocol", () => {
    const body = updateBody(must(github), {
      ...valuesFromProvider(must(github)),
      displayName: "Other",
    });

    expect(body).not.toHaveProperty("slug");
    expect(body).not.toHaveProperty("protocol");
    expect(body).not.toHaveProperty("preset");
  });

  test("discovery fills the issuer and endpoints but leaves everything else alone", () => {
    const applied = applyDiscovery(
      { ...emptyValues(), clientId: "kept", scopes: ["openid"] },
      {
        issuerUrl: "https://sso.example.test/",
        endpoints: {
          authorization: "https://sso.example.test/a",
          token: "https://sso.example.test/t",
          userinfo: null,
          jwks: "https://sso.example.test/j",
        },
        scopesSupported: ["openid", "email"],
        tokenEndpointAuthMethodsSupported: [],
      },
    );

    expect(applied).toMatchObject({
      issuerUrl: "https://sso.example.test/",
      authorizationEndpoint: "https://sso.example.test/a",
      tokenEndpoint: "https://sso.example.test/t",
      userinfoEndpoint: "",
      jwksEndpoint: "https://sso.example.test/j",
      clientId: "kept",
      scopes: ["openid"],
    });
  });

  test("a stored validation summary is split into check and reason codes", () => {
    expect(parseValidationError("userinfo_endpoint:upstream_error,token_endpoint:timeout")).toEqual(
      [
        { name: "userinfo_endpoint", reason: "upstream_error" },
        { name: "token_endpoint", reason: "timeout" },
      ],
    );
    expect(parseValidationError("jwks")).toEqual([{ name: "jwks", reason: "failed" }]);
    expect(parseValidationError(null)).toEqual([]);
    expect(parseValidationError("")).toEqual([]);
  });
});
