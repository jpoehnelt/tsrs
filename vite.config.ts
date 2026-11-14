import { defineConfig } from "vite-plus";

// Keep the existing playground check scope. Compiler fixtures, generated files,
// and vendored sources are checked by the repository's own tooling.
const ignorePatterns = [
  "/*",
  "!/vite.config.ts",
  "!/package.json",
  "!/pnpm-workspace.yaml",
  "!/playground/",
  "/playground/vendor/",
  "/playground/public/",
  "/playground/src/generated/",
  "**/README.md",
];

export default defineConfig({
  fmt: {
    ignorePatterns,
    overrides: [{ files: ["/package.json"], options: { tabWidth: 4, sortPackageJson: false } }],
  },
  lint: {
    ignorePatterns,
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    options: { typeAware: true, typeCheck: true },
  },
  test: { projects: ["playground"] },
});
