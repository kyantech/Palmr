# App shell visual system

The visual rules of the authenticated shell (`apps/web/src/app/layouts/`) and the page conventions that features rendered inside it follow. Structure and behavior are defined by FRONTEND_ARCHITECTURE §§3, 12 and M09; this document covers only presentation.

Everything is expressed with Ant Design tokens (`theme.useToken()`) and inline styles. There are no global stylesheets beyond `antd/dist/reset.css`, no hard-coded colors, and the global `ThemeConfig` in `app/theme/appearance.ts` stays limited to `algorithm` + `colorPrimary`. Component-level tuning uses a scoped `ConfigProvider` next to the component that needs it.

## Layout

| Breakpoint | Navigation | Main area |
|---|---|---|
| `≥ lg` | Sticky `Sider` on `colorBgLayout`, 256 px, collapsible to 72 px | Inset panel: `marginXS` gap, `colorBgContainer`, `colorBorderSecondary` hairline, radius `borderRadiusLG + 4`, `boxShadowTertiary` |
| `< lg` | Header button opens a `Drawer` with the same registry | Full bleed, no inset |
| `xs` | Additionally a fixed bottom tab bar | Full bleed, bottom spacer of 88 px after the footer |

- The header is sticky, 64 px high, translucent (`color-mix` of `colorBgContainer` at 78 %) with `backdrop-filter` blur and a hairline bottom border.
- The main area background is flat `colorBgContainer`: no gradients or decorative glows.
- Page content is centred with `max-width: 1280px`, `paddingXL` inline on desktop and `padding` below `lg`.

## Navigation

`NavigationMenu` scopes these `Menu` tokens: item height 40, radius `borderRadius + 2`, no inline margin, transparent background, `colorTextSecondary` at rest, `colorFillTertiary` on hover, `colorPrimaryBg` + `colorText` when selected, 18 px icons.

- The selected label uses `colorText`, not `colorPrimaryText`: primary text on `colorPrimaryBg` falls below 4.5:1 under the dark algorithm.
- Registry icons are `@gravity-ui/icons` components imported per icon (`import Name from "@gravity-ui/icons/Name"`) and rendered with `aria-hidden`, `focusable="false"` and `1em` size. The menu wraps each in `<span className="anticon">` so Ant Design's icon spacing and collapsed centring apply. Do the same for any icon passed as a `Menu`/`Dropdown` item `icon`.
- Bottom tabs show the icon inside a 56 × 30 pill (`colorPrimaryBg`, icon `colorPrimary`) with the label in `colorText` when selected; every tab is at least 64 px high.

## Header account menu

- Avatar: initials on a `colorPrimary → colorPrimaryActive` gradient with a `colorPrimaryBorder` ring; an uploaded avatar replaces it.
- Trigger: name and e-mail at `sm` and above, avatar only at `xs`. The accessible name is always `user.menu.label`.
- The popup (`popupRender`) prepends a non-interactive account card to the menu. Only actions are `menuitem`s; the sign-out item is `danger`.

## Footer

The version is a small monospace chip (`fontFamilyCode`, `colorFillQuaternary`); its text content must stay the bare version string. `Powered by Palmr` uses `colorTextTertiary` and stays pinned to the inline end. Both are separated from content by a `colorSplit` hairline. Visibility rules are unchanged (Decision 93).

## Page conventions

- Every route renders exactly one `h1` as its page title: `fontWeight: 700`, `letterSpacing: -0.025em`, `lineHeight: 1.15`, size `clamp(fontSizeHeading3, 2.4vw + 12px, fontSizeHeading1)`.
- Do not add page-level padding or max-width inside a feature route; the shell provides both.
- Loading states use skeletons shaped like the content that arrives; empty states use icon + title + description + action.

## Settings pages

`/settings/*` renders one `h1` ("Settings") from `SettingsLayout`; each section is an `h2` with a one-line secondary description.

- Section navigation is local to the page and never duplicates the primary registry. At `≥ md` it is a 220 px inline `Menu` (selected item `colorFillSecondary`) beside a content column capped at 760 px; below `md` it is a horizontally scrollable row of pill links above the content, so no section is hidden behind an overflow menu.
- Forms keep the vertical layout of the auth screens. Password fields sit in a 480 px column; the language `Select` is capped at 360 px.
- Appearance changes save on selection. The status slot next to the section title reads "Saving…" then "Saved"; a failed save shows the mapped error and the controls return to the server's value.
- Destructive session actions confirm with a `Popconfirm` that states the consequence. Ending the current session is labelled "Sign out", never "Revoke".

## Admin pages

`/admin/*` is guarded by the full authenticated chain plus `RequireAdmin` and renders `AppShell` + `AdminLayout`: one `h1` ("Administration"), a horizontally scrollable row of pill links (Users, Security, SMTP) and one `h2` per page. Users and Invites share `/admin/users`; the segmented control writes `?view=users|invites`.

- Server-driven lists own their state in the URL: `q`, `role`, `status`, `sort`, `cursor`, `limit` (users) and `status`, `cursor`, `limit` (invites). Cursors are opaque; paging uses Next, a trail kept in `location.state` for Previous, and First page. Any filter, sort or page-size change drops the cursor.
- Route loaders only prime the Query cache and never block navigation or throw. They read the cached `/auth/me` and skip priming unless the user is an unrestricted Admin, because loaders run before the guard components. The Query client reaches loaders through the router context (`routeContext.ts`).
- Every Admin mutation declares its own invalidation set in `features/admin/api/mutations.ts`. Components never refresh data themselves and never catch `AUTH_RECENT_AUTH_REQUIRED`: the global challenge replays the mutation, so success handlers are passed to `mutate`/the hook and must be safe to run after the replay.
- One-time secrets (temporary password, invite link) are handed to component state from inside `mutationFn`, never returned as mutation data, so they are not kept in the mutation cache. `OneTimeSecretModal` is the only place they are rendered, with an explicit Copy button; closing it drops the value.
- Settings forms save one group each and send only the keys that changed. Nullable settings (`null` = Unlimited / no maximum) are distinct from `0`. SMTP password is write-only: the field is blank on load, omitted when untouched, sent when typed and `null` only through the explicit Clear action.
- Quota override has three states (`inherit`, `unlimited`, `bytes`). The user detail response does not carry the override mode, so the form infers it from `quotaBytes`, `effectiveQuotaBytes` and the instance default; the mode returned by a save is authoritative.

## Visual validation

Component tests cannot see layout. For any change to the shell, run the real backend against a throwaway data directory and capture desktop (1440 × 900), tablet (834 × 1112, drawer open) and phone (390 × 844) in both colour schemes, plus the expanded account menu and the collapsed sider.

While iterating, use the Vite dev server with hot reload. It serves the SPA on `http://localhost:5173` and proxies `/api` to the backend (`PALMR_DEV_API`, default `http://127.0.0.1:5487`). `PALMR_BASE_URL` must be the Vite origin so the backend accepts the browser's requests:

```sh
PALMR_DATA_DIR=$(mktemp -d) PALMR_BASE_URL=http://localhost:5173 cargo run -p palmr-server --bin palmr
pnpm --filter web dev
```

If port 5487 is taken, start the backend with `PALMR_PORT=5488` and the web server with `PALMR_DEV_API=http://127.0.0.1:5488`. The dev server does not receive the backend's head injection (CSP nonce, favicon), so confirm the final result against the binary serving the built SPA:

```sh
pnpm --filter web build
cargo build -p palmr-server --bin palmr --features dev-assets
PALMR_DATA_DIR=$(mktemp -d) PALMR_PORT=5599 PALMR_BASE_URL=http://127.0.0.1:5599 target/debug/palmr
```

With `dev-assets` the server reads `apps/web/dist` from disk, but the asset manifest is read at startup: restart the binary after each web build.
