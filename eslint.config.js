import eslint from "@eslint/js";
import globals from "globals";

export default [
  eslint.configs.recommended,
  {
    files: ["action/**/*.js", "eslint.config.js"],
    languageOptions: {
      ecmaVersion: 2025,
      sourceType: "module",
      globals: globals.node,
    },
  },
];
