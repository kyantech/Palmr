import eslint from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: ["playwright-report", "test-results"],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  {
    languageOptions: {
      globals: globals.node,
    },
    rules: {
      "no-restricted-imports": [
        "error",
        {
          paths: ["msw"],
          patterns: ["msw/*"],
        },
      ],
      "no-restricted-syntax": [
        "error",
        {
          selector: "CallExpression[callee.property.name='waitForTimeout']",
          message: "Synchronize on observable application state.",
        },
      ],
    },
  },
);
