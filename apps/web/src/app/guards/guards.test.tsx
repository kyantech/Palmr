import { screen } from "@testing-library/react";
import { type RouteObject, useLocation } from "react-router";
import { afterEach, describe, expect, test } from "vitest";
import { clearMfaChallenge, mfaChallengeStore } from "../../features/auth";
import { bootstrapFixture, meFixture } from "../../test/bootFixtures";
import { renderRouter } from "../../test/renderRouter";
import type { BootState } from "../bootstrap/bootState";
import type { Me } from "../bootstrap/queries";
import { PATHS } from "../router/paths";
import { ADMIN_GUARDS, adminRoutes, AUTHENTICATED_GUARDS, type Guard, guardChain } from "./chain";
import { RequireAdmin } from "./RequireAdmin";
import { RequireAnonymous } from "./RequireAnonymous";
import { RequireAuth } from "./RequireAuth";
import { RequireEnrolled2FA } from "./RequireEnrolled2FA";
import { RequireMfaPending } from "./RequireMfaPending";
import { RequirePending2faEnrollment } from "./RequirePending2faEnrollment";
import { RequirePendingPasswordChange } from "./RequirePendingPasswordChange";
import { RequireNoPendingPasswordChange } from "./RequireNoPendingPasswordChange";
import { RequireSetup } from "./RequireSetup";
import { RequireSetupIncomplete } from "./RequireSetupIncomplete";

const NAMES = new Map<Guard, string>([
  [RequireSetup, "setup"],
  [RequireAuth, "auth"],
  [RequireNoPendingPasswordChange, "password"],
  [RequireEnrolled2FA, "2fa"],
  [RequireAdmin, "role"],
]);

function Landing({ label }: { label: string }) {
  const { pathname, search } = useLocation();
  return <output data-testid="landing">{`${label} ${pathname}${search}`}</output>;
}

const destinations: RouteObject[] = [
  { path: PATHS.setup, element: <Landing label="setup" /> },
  { path: PATHS.login, element: <Landing label="login" /> },
  { path: PATHS.forcedPasswordChange, element: <Landing label="forced-password" /> },
  { path: PATHS.enrollTwoFactor, element: <Landing label="enroll-2fa" /> },
  { path: PATHS.overview, element: <Landing label="overview" /> },
];

function instrumented(guards: readonly Guard[], log: string[]): Guard[] {
  return guards.map((Inner) => {
    const name = NAMES.get(Inner) ?? "unknown";
    function Recording() {
      log.push(name);
      return <Inner />;
    }
    return Recording;
  });
}

function state(me: Me | null, setupCompleted = true): BootState {
  return { bootstrap: bootstrapFixture({ setupCompleted }), me };
}

async function landing() {
  return (await screen.findByTestId("landing")).textContent;
}

function unique(log: string[]): string[] {
  return [...new Set(log)];
}

describe("component_guard_order", () => {
  test("the production chains are declared setup → auth → password → 2FA → role", () => {
    expect(AUTHENTICATED_GUARDS.map((guard) => NAMES.get(guard))).toEqual([
      "setup",
      "auth",
      "password",
      "2fa",
    ]);
    expect(ADMIN_GUARDS.map((guard) => NAMES.get(guard))).toEqual([
      "setup",
      "auth",
      "password",
      "2fa",
      "role",
    ]);
  });

  async function run(bootState: BootState, target = "/admin-area") {
    const log: string[] = [];
    const routes: RouteObject[] = [
      ...destinations,
      ...guardChain(instrumented(ADMIN_GUARDS, log), [
        { path: "/admin-area", element: <Landing label="admin-content" /> },
      ]),
    ];
    const { router } = await renderRouter(routes, { state: bootState, initialEntries: [target] });
    return { log, router };
  }

  test("setup beats auth: setup incomplete + anonymous goes to /setup, never /login", async () => {
    const { log, router } = await run(state(null, false));

    expect(await landing()).toBe("setup /setup");
    expect(unique(log)).toEqual(["setup"]);
    expect(router.state.historyAction).toBe("REPLACE");
  });

  test("setup beats auth even with a session present", async () => {
    const { log } = await run(state(meFixture({ role: "admin" }), false));

    expect(await landing()).toBe("setup /setup");
    expect(unique(log)).toEqual(["setup"]);
  });

  test("auth beats restrictions: anonymous goes to /login before any restriction guard runs", async () => {
    const { log } = await run(state(null));

    expect(await landing()).toBe(`login /login?next=${encodeURIComponent("/admin-area")}`);
    expect(unique(log)).toEqual(["setup", "auth"]);
  });

  test("forced password change is evaluated before 2FA enrolment", async () => {
    const { log } = await run(
      state(meFixture({ role: "admin", restriction: "must_change_password" })),
    );

    expect(await landing()).toBe(
      `forced-password /login/forced-password-change?next=${encodeURIComponent("/admin-area")}`,
    );
    expect(unique(log)).toEqual(["setup", "auth", "password"]);
  });

  test("2FA beats role: an Admin that must enrol never reaches Admin content", async () => {
    const { log } = await run(
      state(meFixture({ role: "admin", restriction: "mfa_enrollment_required" })),
    );

    expect(await landing()).toBe(
      `enroll-2fa /login/enroll-2fa?next=${encodeURIComponent("/admin-area")}`,
    );
    expect(unique(log)).toEqual(["setup", "auth", "password", "2fa"]);
    expect(screen.queryByText(/admin-content/)).toBeNull();
  });

  test("role is last: an unrestricted User reaches the role guard and gets 403", async () => {
    const { log, router } = await run(state(meFixture({ role: "user" })));

    expect(await screen.findByRole("heading", { level: 1, name: "Access denied" })).toBeDefined();
    expect(unique(log)).toEqual(["setup", "auth", "password", "2fa", "role"]);
    expect(router.state.location.pathname).toBe("/admin-area");
  });

  test("an unrestricted Admin passes every guard", async () => {
    const { log } = await run(state(meFixture({ role: "admin" })));

    expect(await landing()).toBe("admin-content /admin-area");
    expect(unique(log)).toEqual(["setup", "auth", "password", "2fa", "role"]);
  });
});

describe("guards", () => {
  test("component_require_admin_renders_403", async () => {
    const { router } = await renderRouter(
      [...destinations, ...adminRoutes([{ path: "/admin", element: <p>admin</p> }])],
      { state: state(meFixture({ role: "user" })), initialEntries: ["/admin"] },
    );

    const heading = await screen.findByRole("heading", { level: 1, name: "Access denied" });
    expect(heading).toBeDefined();
    expect(screen.getByText("403")).toBeDefined();
    expect(screen.getByRole("button", { name: "Go to Overview" })).toBeDefined();
    expect(screen.queryByText("admin")).toBeNull();
    expect(router.state.location.pathname).toBe("/admin");
  });

  test("RequireAdmin owns only the role predicate", async () => {
    await renderRouter(
      [
        {
          element: <RequireAdmin />,
          children: [{ path: "/admin", element: <Landing label="admin" /> }],
        },
      ],
      {
        state: state(meFixture({ role: "admin", restriction: "must_change_password" }), false),
        initialEntries: ["/admin"],
      },
    );

    expect(await landing()).toBe("admin /admin");
  });

  test("RequireAuth keeps pathname + search in next, without the basename or an origin", async () => {
    await renderRouter(
      [
        ...destinations,
        {
          element: <RequireAuth />,
          children: [{ path: "/files/:id", element: <p>files</p> }],
        },
      ],
      { state: state(null), initialEntries: ["/files/abc?sort=name"] },
    );

    expect(await landing()).toBe(`login /login?next=${encodeURIComponent("/files/abc?sort=name")}`);
  });

  test.each([
    ["/login?next=%2Ffiles%2Fabc%3Fsort%3Dname", "files /files/abc?sort=name"],
    ["/login?next=%2F%2Fevil.example", "overview /overview"],
    ["/login?next=https%3A%2F%2Fevil.example", "overview /overview"],
    ["/login?next=%2F%5Cevil.example", "overview /overview"],
    ["/login", "overview /overview"],
  ])("RequireAnonymous sends a signed-in visitor at %s to %s", async (entry, expected) => {
    await renderRouter(
      [
        ...destinations.filter((route) => route.path !== PATHS.login),
        { path: "/files/:id", element: <Landing label="files" /> },
        { element: <RequireAnonymous />, children: [{ path: "/login", element: <p>login</p> }] },
      ],
      { state: state(meFixture()), initialEntries: [entry] },
    );

    expect(await landing()).toBe(expected);
  });

  test("RequireAnonymous renders its outlet for an anonymous visitor", async () => {
    await renderRouter(
      [
        {
          element: <RequireAnonymous />,
          children: [{ path: "/login", element: <Landing label="anonymous" /> }],
        },
      ],
      { state: state(null), initialEntries: ["/login"] },
    );

    expect(await landing()).toBe("anonymous /login");
  });

  test.each([
    [false, "setup-form /setup"],
    [true, "root /"],
  ])(
    "RequireSetupIncomplete with setupCompleted=%s lands on %s",
    async (setupCompleted, expected) => {
      await renderRouter(
        [
          { path: "/", element: <Landing label="root" /> },
          {
            element: <RequireSetupIncomplete />,
            children: [{ path: "/setup", element: <Landing label="setup-form" /> }],
          },
        ],
        { state: state(null, setupCompleted), initialEntries: ["/setup"] },
      );

      expect(await landing()).toBe(expected);
    },
  );

  test.each([
    [null, "protected /protected"],
    ["must_change_password", "forced-password /login/forced-password-change?next=%2Fprotected"],
    ["mfa_enrollment_required", "enroll-2fa /login/enroll-2fa?next=%2Fprotected"],
  ] as const)(
    "restriction %s routes an authenticated user to %s",
    async (restriction, expected) => {
      await renderRouter(
        [
          ...destinations,
          ...guardChain(AUTHENTICATED_GUARDS, [
            { path: "/protected", element: <Landing label="protected" /> },
          ]),
        ],
        { state: state(meFixture({ restriction })), initialEntries: ["/protected"] },
      );

      expect(await landing()).toBe(expected);
    },
  );
});

describe("M10 lock and challenge guards", () => {
  afterEach(() => {
    clearMfaChallenge();
  });

  function challenge(deadline: number) {
    mfaChallengeStore.setState({
      challenge: {
        mfaToken: "memory-only-token",
        expiresAt: "2026-09-28T00:05:00Z",
        methods: ["totp", "backup_code"],
        trustedDeviceOffered: false,
        deadline,
      },
    });
  }

  async function renderMfaGuard(entry: string) {
    return renderRouter(
      [
        { path: PATHS.login, element: <Landing label="login" /> },
        {
          element: <RequireMfaPending />,
          children: [{ path: PATHS.twoFactor, element: <Landing label="2fa" /> }],
        },
      ],
      { state: state(null), initialEntries: [entry] },
    );
  }

  test("RequireMfaPending without an in-memory challenge replaces /login/2fa with /login and keeps next", async () => {
    const { router } = await renderMfaGuard(`/login/2fa?next=${encodeURIComponent("/files/a")}`);

    expect(await landing()).toBe(`login /login?next=${encodeURIComponent("/files/a")}`);
    expect(router.state.historyAction).toBe("REPLACE");
  });

  test("RequireMfaPending renders the challenge route while the challenge is live", async () => {
    challenge(Date.now() + 60_000);
    await renderMfaGuard("/login/2fa");

    expect(await landing()).toBe("2fa /login/2fa");
  });

  test("RequireMfaPending clears a locally expired challenge and returns to /login", async () => {
    challenge(Date.now() - 1);
    await renderMfaGuard("/login/2fa");

    expect(await landing()).toBe("login /login");
    expect(mfaChallengeStore.getState().challenge).toBeNull();
  });

  const lockRoutes: RouteObject[] = [
    ...destinations,
    { path: "/files/:id", element: <Landing label="files" /> },
    {
      element: <RequirePendingPasswordChange />,
      children: [{ path: "/lock/password", element: <Landing label="password-lock" /> }],
    },
    {
      element: <RequirePending2faEnrollment />,
      children: [{ path: "/lock/2fa", element: <Landing label="2fa-lock" /> }],
    },
  ];

  test.each([
    ["/lock/password", "must_change_password", "password-lock /lock/password?next=%2Ffiles%2Fa"],
    ["/lock/password", "mfa_enrollment_required", "enroll-2fa /login/enroll-2fa?next=%2Ffiles%2Fa"],
    ["/lock/password", null, "files /files/a"],
    ["/lock/2fa", "mfa_enrollment_required", "2fa-lock /lock/2fa?next=%2Ffiles%2Fa"],
    [
      "/lock/2fa",
      "must_change_password",
      "forced-password /login/forced-password-change?next=%2Ffiles%2Fa",
    ],
    ["/lock/2fa", null, "files /files/a"],
  ] as const)(
    "%s with restriction %s follows the server restriction to %s",
    async (path, restriction, expected) => {
      await renderRouter(lockRoutes, {
        state: state(meFixture({ restriction })),
        initialEntries: [`${path}?next=${encodeURIComponent("/files/a")}`],
      });

      expect(await landing()).toBe(expected);
    },
  );

  test("a lifted restriction with no next lands on /overview and never follows an unsafe next", async () => {
    await renderRouter(lockRoutes, {
      state: state(meFixture({ restriction: null })),
      initialEntries: [`/lock/password?next=${encodeURIComponent("//evil.example")}`],
    });

    expect(await landing()).toBe("overview /overview");
  });
});
