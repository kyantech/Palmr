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
- Registry icons are plain SVG components that ignore `className`. The menu wraps each in `<span className="anticon">` so Ant Design's icon spacing and collapsed centring apply. Do the same for any custom SVG passed as a `Menu`/`Dropdown` item `icon`.
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
