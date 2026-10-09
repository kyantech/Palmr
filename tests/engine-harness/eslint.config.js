import eslint from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
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
          patterns: [
            {
              group: [
                "react",
                "react-dom",
                "react-dom/*",
                "antd",
                "antd/*",
                "react-router",
                "react-router/*",
              ],
              message:
                "The engine harness is framework-agnostic and test-only.",
            },
          ],
        },
      ],
    },
  },
);
