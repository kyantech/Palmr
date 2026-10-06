import { http, HttpResponse } from "msw";
import type { components } from "../shared/api/schema";
import { errorEnvelope } from "./bootFixtures";
import { server } from "./server";
import type { SettingsServerState } from "./settingsServer";

type IdentityLinkItem = components["schemas"]["IdentityLinkItem"];

const API = "*/api/v1";

export const AUTHORIZATION_URL =
  "https://idp.example.test/authorize?response_type=code&client_id=palmr&state=server-state";

export function identityLink(
  overrides: Partial<IdentityLinkItem> & { id: string },
): IdentityLinkItem {
  return {
    providerSlug: "authentik",
    providerDisplayName: "Company SSO",
    externalSubject: "subject-that-must-not-render",
    emailAtLink: "ada@example.test",
    linkedAt: "2026-09-10T08:00:00Z",
    lastUsedAt: "2026-09-27T08:00:00Z",
    ...overrides,
  };
}

export interface IdentityServerOptions {
  links?: IdentityLinkItem[];
  linkFailure?: () => Response;
  unlinkFailure?: () => Response;
}

export interface IdentityServerState {
  links: IdentityLinkItem[];
  listCalls: number;
  linkStarts: string[];
  unlinkAttempts: string[];
  unlinked: string[];
}

export function installIdentityServer(
  settings: SettingsServerState,
  { links = [], linkFailure, unlinkFailure }: IdentityServerOptions = {},
): IdentityServerState {
  const state: IdentityServerState = {
    links,
    listCalls: 0,
    linkStarts: [],
    unlinkAttempts: [],
    unlinked: [],
  };
  const recent = () =>
    settings.recentAuth ? null : errorEnvelope("AUTH_RECENT_AUTH_REQUIRED", 403, "req-recent-auth");
  server.use(
    http.get(`${API}/identity-links`, () => {
      state.listCalls += 1;
      return HttpResponse.json({
        items: state.links,
        nextCursor: null,
        totalCount: state.links.length,
      });
    }),
    http.post(`${API}/auth/providers/:slug/link`, ({ params }) => {
      const slug = String(params.slug);
      state.linkStarts.push(slug);
      const blocked = recent();
      if (blocked !== null) {
        return blocked;
      }
      if (linkFailure !== undefined) {
        return linkFailure();
      }
      if (state.links.some((link) => link.providerSlug === slug)) {
        return errorEnvelope("PROVIDER_IDENTITY_ALREADY_LINKED", 409, "req-already-linked");
      }
      return HttpResponse.json({ authorizationUrl: AUTHORIZATION_URL });
    }),
    http.delete(`${API}/identity-links/:id`, ({ params }) => {
      const id = String(params.id);
      state.unlinkAttempts.push(id);
      const blocked = recent();
      if (blocked !== null) {
        return blocked;
      }
      if (unlinkFailure !== undefined) {
        return unlinkFailure();
      }
      if (!state.links.some((link) => link.id === id)) {
        return errorEnvelope("PROVIDER_LINK_NOT_FOUND", 404, "req-missing-link");
      }
      state.unlinked.push(id);
      state.links = state.links.filter((link) => link.id !== id);
      settings.me = null;
      return new HttpResponse(null, { status: 204 });
    }),
  );
  return state;
}
