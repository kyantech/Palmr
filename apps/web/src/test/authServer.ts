import { http, HttpResponse } from "msw";
import type { components } from "../shared/api/schema";
import { errorEnvelope } from "./bootFixtures";
import { server } from "./server";
import type { SettingsServerState } from "./settingsServer";

type TwoFactorStatus = components["schemas"]["TwoFactorStatus"];
type TrustedDeviceItem = components["schemas"]["TrustedDeviceItem"];
type TrustedDevicePolicy = components["schemas"]["TrustedDevicePolicy"];

const API = "*/api/v1";

export const MFA_TOKEN = "mfa-7c1e2f9a4b6d8e0f1a3c5e7b9d1f3a5c";
export const TOTP_SECRET = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
export const OTPAUTH_URI = `otpauth://totp/Palmr:ada@example.test?secret=${TOTP_SECRET}&issuer=Palmr&algorithm=SHA1&digits=6&period=30`;
export const ENROLLMENT_ID = "3f9c1d2ab47e5f60718293a4b5c6d7e8";
export const BACKUP_CODES = [
  "A3F2-QK7L-QW7E-M4ZP",
  "B7HD-2MNQ-4RST-UVWX",
  "C2DE-FGHJ-KLMN-PQRS",
  "D3EF-GHJK-LMNP-QRST",
  "E4FG-HJKL-MNPQ-RSTU",
  "F5GH-JKLM-NPQR-STUV",
  "G6HJ-KLMN-PQRS-TUVW",
  "H7JK-LMNP-QRST-UVWX",
  "J2KL-MNPQ-RSTU-VWXY",
  "K3LM-NPQR-STUV-WXYZ",
];
export const CURRENT_DEVICE_ID = "019a0000-0000-7000-8000-00000000d001";
export const OTHER_DEVICE_ID = "019a0000-0000-7000-8000-00000000d002";

export function mfaChallengeEnvelope({
  token = MFA_TOKEN,
  trustedDeviceOffered = true,
  methods = ["totp", "backup_code"],
  expiresInMs = 300_000,
}: {
  token?: string;
  trustedDeviceOffered?: boolean;
  methods?: string[];
  expiresInMs?: number;
} = {}) {
  return HttpResponse.json(
    {
      error: {
        code: "AUTH_2FA_REQUIRED",
        message: "a second factor is required",
        requestId: "req-mfa",
        details: {
          mfaToken: token,
          expiresAt: new Date(Date.now() + expiresInMs).toISOString(),
          methods,
          trustedDeviceOffered,
        },
      },
    },
    { status: 401, headers: { "X-Request-Id": "req-mfa" } },
  );
}

export function twoFactorStatus(overrides: Partial<TwoFactorStatus> = {}): TwoFactorStatus {
  return {
    enabled: false,
    enrolledAt: null,
    backupCodesRemaining: 0,
    requiredByPolicy: false,
    canDisable: true,
    ...overrides,
  };
}

export function trustedDevice(
  overrides: Partial<TrustedDeviceItem> & { id: string },
): TrustedDeviceItem {
  return {
    label: "Firefox on Windows",
    ipAtEnrollment: "198.51.100.7",
    createdAt: "2026-09-20T08:00:00Z",
    lastSeenAt: "2026-09-27T08:00:00Z",
    expiresAt: "2026-10-20T08:00:00Z",
    isCurrent: false,
    ...overrides,
  };
}

export interface TwoFactorServerState {
  status: TwoFactorStatus;
  devices: TrustedDeviceItem[];
  policy: TrustedDevicePolicy;
  calls: { status: number; devices: number };
  enrollCalls: number;
  verifyBodies: unknown[];
  disableCalls: number;
  regenerateCalls: number;
  revokedDevices: string[];
  revokeAllCalls: number;
}

export interface TwoFactorServerOptions {
  status?: TwoFactorStatus;
  devices?: TrustedDeviceItem[];
  policy?: TrustedDevicePolicy;
  verify?: (body: unknown) => Response | null;
}

export function installTwoFactorServer(
  settings: SettingsServerState,
  { status = twoFactorStatus(), devices = [], policy, verify }: TwoFactorServerOptions = {},
): TwoFactorServerState {
  const state: TwoFactorServerState = {
    status,
    devices,
    policy: policy ?? { enabled: true, durationDays: 30 },
    calls: { status: 0, devices: 0 },
    enrollCalls: 0,
    verifyBodies: [],
    disableCalls: 0,
    regenerateCalls: 0,
    revokedDevices: [],
    revokeAllCalls: 0,
  };
  const recent = () =>
    settings.recentAuth ? null : errorEnvelope("AUTH_RECENT_AUTH_REQUIRED", 403, "req-recent-auth");
  const restrictedEnrollment = () => settings.me?.restriction === "mfa_enrollment_required";

  server.use(
    http.get(`${API}/auth/2fa`, () => {
      state.calls.status += 1;
      return HttpResponse.json(state.status);
    }),
    http.post(`${API}/auth/2fa/enroll`, () => {
      state.enrollCalls += 1;
      const blocked = restrictedEnrollment() ? null : recent();
      if (blocked !== null) {
        return blocked;
      }
      return HttpResponse.json({
        enrollmentId: ENROLLMENT_ID,
        otpauthUri: OTPAUTH_URI,
        secretBase32: TOTP_SECRET,
        expiresAt: new Date(Date.now() + 600_000).toISOString(),
      });
    }),
    http.post(`${API}/auth/2fa/enroll/verify`, async ({ request }) => {
      const body = await request.json();
      state.verifyBodies.push(body);
      const override = verify?.(body) ?? null;
      if (override !== null) {
        return override;
      }
      state.status = twoFactorStatus({
        enabled: true,
        enrolledAt: "2026-09-28T00:01:00Z",
        backupCodesRemaining: BACKUP_CODES.length,
        requiredByPolicy: state.status.requiredByPolicy,
        canDisable: !state.status.requiredByPolicy,
      });
      if (settings.me !== null) {
        settings.me = {
          ...settings.me,
          restriction: null,
          capabilities: { ...settings.me.capabilities, twoFactorEnabled: true },
        };
      }
      return HttpResponse.json({ backupCodes: BACKUP_CODES, generatedAt: "2026-09-28T00:01:00Z" });
    }),
    http.post(`${API}/auth/2fa/disable`, () => {
      state.disableCalls += 1;
      const blocked = recent();
      if (blocked !== null) {
        return blocked;
      }
      if (state.status.requiredByPolicy) {
        return errorEnvelope("TOTP_REQUIRED_BY_POLICY", 403, "req-policy");
      }
      state.status = twoFactorStatus();
      state.devices = [];
      settings.me = null;
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${API}/auth/2fa/backup-codes/regenerate`, () => {
      state.regenerateCalls += 1;
      const blocked = recent();
      if (blocked !== null) {
        return blocked;
      }
      state.status = { ...state.status, backupCodesRemaining: BACKUP_CODES.length };
      return HttpResponse.json({
        backupCodes: BACKUP_CODES.map((code) => code.replace(/^./, "Z")),
        generatedAt: "2026-09-28T00:02:00Z",
      });
    }),
    http.get(`${API}/auth/trusted-devices`, () => {
      state.calls.devices += 1;
      return HttpResponse.json({
        items: state.devices,
        nextCursor: null,
        totalCount: state.devices.length,
        policy: state.policy,
      });
    }),
    http.delete(`${API}/auth/trusted-devices/:id`, ({ params }) => {
      const blocked = recent();
      if (blocked !== null) {
        return blocked;
      }
      const id = String(params.id);
      if (!state.devices.some((device) => device.id === id)) {
        return errorEnvelope("TRUSTED_DEVICE_NOT_FOUND", 404, "req-missing-device");
      }
      state.revokedDevices.push(id);
      state.devices = state.devices.filter((device) => device.id !== id);
      return new HttpResponse(null, { status: 204 });
    }),
    http.delete(`${API}/auth/trusted-devices`, () => {
      state.revokeAllCalls += 1;
      const blocked = recent();
      if (blocked !== null) {
        return blocked;
      }
      state.devices = [];
      return new HttpResponse(null, { status: 204 });
    }),
  );
  return state;
}
