import js from "@eslint/js";
import prettier from "eslint-config-prettier";
import boundaries from "eslint-plugin-boundaries";
import i18next from "eslint-plugin-i18next";
import i18nextDefaults from "eslint-plugin-i18next/lib/options/defaults.js";
import importPlugin from "eslint-plugin-import";
import jsxA11y from "eslint-plugin-jsx-a11y";
import reactHooks from "eslint-plugin-react-hooks";
import { defineConfig, globalIgnores } from "eslint/config";
import globals from "globals";
import tseslint from "typescript-eslint";

const LINT_FIXTURES = "src/test/lint-fixtures";
const SOURCE_ROOTS = ["src", LINT_FIXTURES];

const PROHIBITED_DIRECTORIES = [
  "components",
  "services",
  "hooks",
  "utils",
  "lib",
  "helpers",
  "common",
  "store",
];

const inRoots = (...patterns) =>
  SOURCE_ROOTS.flatMap((root) => patterns.map((pattern) => `${root}/${pattern}`));

const USER_FACING_ATTRIBUTES = [
  "title",
  "alt",
  "placeholder",
  "label",
  "aria-label",
  "aria-description",
  "aria-roledescription",
  "aria-valuetext",
  "tooltip",
  "okText",
  "cancelText",
  "description",
  "message",
  "content",
  "extra",
  "help",
  "emptyText",
];

const BRAND_WORDS = ["Palmr"];

const restrictedPathZones = SOURCE_ROOTS.flatMap((root) => [
  {
    target: `./${root}/transfer-engine`,
    from: `./${root}`,
    except: ["./transfer-engine"],
    message: "I3: transfer-engine/ imports nothing else from src/ (FRONTEND_ARCHITECTURE §2.5).",
  },
  ...PROHIBITED_DIRECTORIES.map((directory) => ({
    target: `./${root}`,
    from: `./${root}/${directory}`,
    message: `src/${directory}/ is a prohibited directory (FRONTEND_ARCHITECTURE §2.4).`,
  })),
]);

export default defineConfig([
  globalIgnores(["dist", LINT_FIXTURES, "src/shared/api/schema.d.ts"]),

  {
    files: ["**/*.{js,ts,tsx}"],
    extends: [
      js.configs.recommended,
      tseslint.configs.strictTypeChecked,
      tseslint.configs.stylisticTypeChecked,
    ],
    languageOptions: {
      parserOptions: {
        projectService: { allowDefaultProject: ["eslint.config.js"] },
        tsconfigRootDir: import.meta.dirname,
      },
    },
  },

  {
    files: ["**/*.js"],
    extends: [tseslint.configs.disableTypeChecked],
  },

  {
    files: ["*.{js,ts}"],
    languageOptions: { globals: globals.node },
  },

  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [reactHooks.configs.flat["recommended-latest"], jsxA11y.flatConfigs.recommended],
    languageOptions: { globals: globals.browser },
    plugins: { boundaries, import: importPlugin },
    settings: {
      "import/resolver": {
        typescript: { project: ["./tsconfig.app.json", "./tsconfig.node.json"] },
      },
      "boundaries/elements": SOURCE_ROOTS.flatMap((root) => [
        { type: "app", pattern: `${root}/app` },
        { type: "feature", pattern: `${root}/features/*`, capture: ["feature"] },
        { type: "engine", pattern: `${root}/transfer-engine` },
        { type: "shared", pattern: `${root}/shared` },
      ]),
    },
    rules: {
      "boundaries/dependencies": [
        "error",
        {
          default: "disallow",
          policies: [
            {
              from: { element: { type: "app" } },
              allow: { to: { element: { types: ["app", "engine", "shared"] } } },
            },
            {
              from: { element: { type: "app" } },
              allow: { to: { element: { type: "feature", fileInternalPath: "index.ts" } } },
            },
            {
              from: { element: { type: "feature" } },
              allow: { to: { element: { type: "shared" } } },
            },
          ],
        },
      ],
      "import/no-restricted-paths": ["error", { zones: restrictedPathZones }],
    },
  },

  {
    files: inRoots("features/*/components/**/*.{ts,tsx}", "features/*/routes/**/*.{ts,tsx}"),
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["**/shared/api", "**/shared/api/**"],
              importNames: ["apiFetch"],
              message: "I5: components and routes call the API through features/<x>/api/.",
            },
          ],
        },
      ],
    },
  },

  {
    files: inRoots("features/*/components/**/*.tsx", "app/layouts/**/*.tsx"),
    plugins: { i18next },
    rules: {
      "i18next/no-literal-string": [
        "error",
        {
          mode: "jsx-only",
          "jsx-attributes": { include: USER_FACING_ATTRIBUTES },
          words: { exclude: [...i18nextDefaults.words.exclude, ...BRAND_WORDS] },
        },
      ],
    },
  },

  {
    files: inRoots("transfer-engine/**/*.ts"),
    rules: {
      "no-restricted-imports": [
        "error",
        {
          paths: [
            { name: "jszip", message: "JSZip is forbidden (TRANSFER_ENGINE §1.7)." },
            { name: "react", message: "I3: transfer-engine/ is framework-agnostic." },
            { name: "react-dom", message: "I3: transfer-engine/ is framework-agnostic." },
            { name: "antd", message: "I3: transfer-engine/ is framework-agnostic." },
          ],
          patterns: [
            {
              group: ["react/*", "react-dom/*"],
              message: "I3: transfer-engine/ is framework-agnostic.",
            },
            {
              group: ["antd/*", "@ant-design/*"],
              message: "I3: transfer-engine/ is framework-agnostic.",
            },
          ],
        },
      ],
      "no-restricted-globals": [
        "error",
        { name: "localStorage", message: "Transfer state lives in IndexedDB (ADR 0027 §4)." },
      ],
      "no-restricted-syntax": [
        "error",
        {
          selector: "MemberExpression[property.name='localStorage']",
          message: "Transfer state lives in IndexedDB (ADR 0027 §4).",
        },
        {
          selector: "CallExpression[callee.type='MemberExpression'][callee.property.name='blob']",
          message: "response.blob() buffers the whole body (TRANSFER_ENGINE §1.7).",
        },
      ],
    },
  },

  prettier,
]);
