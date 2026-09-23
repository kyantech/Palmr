# Web quality gates

The frontend quality baseline introduced in M01-T04. It implements the web half of TEST_STRATEGY §9.1 G1 (ESLint, Prettier, TypeScript strict) and G3 (FRONTEND_ARCHITECTURE §2.5 import rules I1–I5), plus the prohibited-directory check of FRONTEND_ARCHITECTURE §2.4.

Run from `v4/`:

```sh
pnpm --filter web lint
pnpm --filter web typecheck
pnpm --filter web test
pnpm --filter web format:check
pnpm --filter web check:structure
```

## Rules

| Rule | Enforced by |
|---|---|
| I1 features never import other features | `boundaries/dependencies`, default `disallow` |
| I2 `app/` imports a feature only through its `index.ts` | `boundaries/dependencies` (`fileInternalPath: "index.ts"`) |
| I3 `transfer-engine/` imports nothing else from `src/` | `boundaries/dependencies` and `import/no-restricted-paths` as a fail-closed backstop. `no-restricted-imports` also blocks `react`, `react-dom`, `antd` and `@ant-design/*` |
| I4 `shared/` imports only `shared/` | `boundaries/dependencies` |
| I5 components and routes do not import `apiFetch` | `no-restricted-imports` on `features/*/{components,routes}/**` |
| No `JSZip`, `response.blob()` or `localStorage` in `transfer-engine/` | `no-restricted-imports`, `no-restricted-syntax`, `no-restricted-globals` (TRANSFER_ENGINE §1.7, ADR 0027 §4). The repository-wide G2 gate is M01-T06 |
| No literal user-facing strings | `i18next/no-literal-string` in `jsx-only` mode on `features/*/components/**` and `app/layouts/**`. It checks JSX text and the user-facing attributes listed in `eslint.config.js`. `Palmr`, upper-case technical tokens, numbers, punctuation and emoji are allowed (FRONTEND_ARCHITECTURE §12.6) |
| No `src/{components,services,hooks,utils,lib,helpers,common,store}/` | `scripts/check-web-structure.mjs` (exit 1 names each offending path) and `import/no-restricted-paths` |

The prohibited names apply only directly under `src/`. `features/<x>/components/`, `features/<x>/hooks/` and `shared/hooks/` are part of the canonical tree.

## Tooling versions

- ESLint stays on 9. `eslint-plugin-import` 2.32 and `eslint-plugin-jsx-a11y` 6.10 do not declare support for ESLint 10. Move to ESLint 10 when both plugins support it.
- `eslint-plugin-boundaries` 7 renamed `element-types` and `entry-point` (the names in FRONTEND_ARCHITECTURE §2.5) to one rule, `boundaries/dependencies`. It covers I1–I4, including the I2 entry point.
- Prettier is pinned exactly because formatting can change between releases.
- The `msw` and `unrs-resolver` install scripts are disabled in `pnpm-workspace.yaml`. Neither is needed: MSW's script only copies the browser worker file, and `unrs-resolver`'s is a fallback for fetching its native binding.

## TypeScript projects

`tsconfig.json` only points to two projects, and both extend `tsconfig.base.json`:

- `tsconfig.app.json` covers application code in `src/`. Its only types are `vite/client`, so Node globals such as `process` and `Buffer` are type errors there.
- `tsconfig.node.json` covers the Vite and Vitest configs, the `*.test.ts(x)` files and `src/test/`, with Node types.

`typecheck` and `build` run `tsc -b`, which checks both projects.

Vitest starts its workers with `--no-experimental-webstorage`. Without it, Node 25+ warns when test tooling touches `localStorage`.

## Lint fixtures

`apps/web/src/test/lint-fixtures/` is a small `app`/`features`/`shared`/`transfer-engine` tree. Some files in it follow the rules and some deliberately break them.

- ESLint's normal run ignores it.
- `tsconfig.app.json` and `tsconfig.node.json` exclude it. The fixtures have their own `tsconfig.json` so type-aware linting still works on them.
- Vite never reaches it from `main.tsx`.

`src/test/lint-fixtures.test.ts` lints the fixtures with the real `eslint.config.js`. It asserts the exact set of rules each file reports, and that valid files report nothing. A new fixture without an expectation fails the test.

`src/test/web-structure.test.ts` runs the structure check against temporary directories, so no prohibited directory ever exists in the source tree.

## Negative check

Run it on a throwaway copy of the tree and never commit it. Last run on 2026-09-23:

| Change | Command | Result |
|---|---|---|
| `src/features/files/components/FileCount.tsx` imports `../../shares` | `pnpm --filter web lint` | `boundaries/dependencies` error, exit 1 |
| `src/features/files/components/EmptyFiles.tsx` renders `<p>No files yet</p>` | `pnpm --filter web lint` | `i18next/no-literal-string` error, exit 1 |
| `mkdir src/utils` | `pnpm --filter web check:structure` | `prohibited directory "src/utils"`, exit 1 |
