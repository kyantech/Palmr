// @vitest-environment node
import { ESLint } from "eslint";
import { relative, resolve } from "node:path";
import { expect, test } from "vitest";

const webRoot = resolve(import.meta.dirname, "../..");
const fixturesRoot = resolve(import.meta.dirname, "lint-fixtures");

const EXPECTED_RULES: Record<string, string[]> = {
  "app/ComposesFeatures.tsx": [],
  "app/ImportsFeatureInternals.tsx": ["boundaries/dependencies"],
  "features/alpha/index.ts": [],
  "features/alpha/components/AlphaTitle.tsx": [],
  "features/alpha/components/CallsApiFetch.tsx": ["no-restricted-imports"],
  "features/alpha/components/ImportsOtherFeature.tsx": ["boundaries/dependencies"],
  "features/alpha/components/LiteralAttribute.tsx": ["i18next/no-literal-string"],
  "features/alpha/components/LiteralText.tsx": ["i18next/no-literal-string"],
  "features/beta/index.ts": [],
  "shared/api/index.ts": [],
  "shared/format.ts": [],
  "shared/ImportsFeature.ts": ["boundaries/dependencies"],
  "transfer-engine/backoff.ts": [],
  "transfer-engine/engine.ts": [],
  "transfer-engine/ImportsJsZip.ts": ["no-restricted-imports"],
  "transfer-engine/ImportsReact.ts": ["no-restricted-imports"],
  "transfer-engine/ImportsShared.ts": ["boundaries/dependencies", "import/no-restricted-paths"],
  "transfer-engine/ReadsWholeBody.ts": ["no-restricted-syntax"],
  "transfer-engine/UsesLocalStorage.ts": ["no-restricted-globals", "no-restricted-syntax"],
};

test("unit_lint_fixtures_report_exactly_the_expected_rules", { timeout: 60_000 }, async () => {
  const eslint = new ESLint({ cwd: webRoot, ignore: false });
  const results = await eslint.lintFiles([`${fixturesRoot}/**/*.{ts,tsx}`]);

  const actual = Object.fromEntries(
    results.map((result) => [
      relative(fixturesRoot, result.filePath),
      [...new Set(result.messages.map((message) => message.ruleId ?? message.message))].sort(),
    ]),
  );

  expect(actual).toEqual(EXPECTED_RULES);
});
