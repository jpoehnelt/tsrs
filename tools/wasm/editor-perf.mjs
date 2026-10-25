// Generic retained-session editor probe; run the same release settings on both revisions.
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { performance } from "node:perf_hooks";
import { createBinarySessionSync, memoryFileSystem } from "../../npm/tsrs-wasm/core.js";
assert.equal(typeof globalThis.gc, "function", "Run Node with --expose-gc");
const [path, output] = process.argv.slice(2);
const bytes = readFileSync(path);
const source = "function convert(value) { return value.repeat(2); } convert('x');";
const config = JSON.stringify({
  compilerOptions: {
    allowJs: true,
    checkJs: true,
    strict: true,
    noEmit: true,
    target: "esnext",
    lib: ["esnext", "dom"],
    types: [],
  },
  files: ["/code.js"],
});
const transport = createBinarySessionSync(new WebAssembly.Module(bytes), memoryFileSystem());
const hash = (value) => createHash("sha256").update(value).digest("hex");
transport.request("openSession");
const snapshot = transport.request("createSnapshot", {
  openProjects: ["/tsconfig.json"],
  fileSystem: { kind: "layer", files: { "/tsconfig.json": config, "/code.js": source } },
});
const project = snapshot.projects.find((project) => project.id === "/tsconfig.json");
assert.ok(project);
const base = {
  snapshot: snapshot.snapshot,
  project: project.id ?? project.project ?? project,
  file: "/code.js",
};
const diagnostics = transport.request("getSemanticDiagnostics", { ...base, files: ["/code.js"] });
const diagnostic = diagnostics.find((d) => d.code === 7006);
assert.ok(diagnostic);
const start = source.indexOf("value)");
const request = {
  textDocument: { uri: "file:///code.js" },
  range: { start: { line: 0, character: start }, end: { line: 0, character: start + 5 } },
  context: {
    diagnostics: [
      {
        code: 7006,
        range: { start: { line: 0, character: start }, end: { line: 0, character: start + 5 } },
        message: "Parameter 'value' implicitly has an 'any' type.",
      },
    ],
    only: ["quickfix"],
  },
};
const memory = [];
const sample = (phase) => {
  globalThis.gc();
  memory.push({ phase, ...process.memoryUsage() });
};
sample("diagnostics");
const fix = () =>
  transport.request("editor", {
    ...base,
    operation: "codeFixes",
    request,
    preferences: { indentSize: 2, tabSize: 2, convertTabsToSpaces: true },
  });
const first = fix();
assert.equal(first[0].fixName, "inferFromUsage");
const answerSha256 = hash(JSON.stringify(first));
sample("firstFix");
const times = [];
for (let i = 0; i < 200; i++) {
  const start = performance.now(),
    value = fix();
  times.push(performance.now() - start);
  assert.equal(hash(JSON.stringify(value)), answerSha256);
}
sample("warmFixes");
transport.request("release", { snapshot: snapshot.snapshot });
transport.request("closeSession");
sample("closed");
const report = {
  compilerSha256: hash(bytes),
  sourceSha256: hash(source),
  configSha256: hash(config),
  diagnosticsSha256: hash(JSON.stringify(diagnostics)),
  answerSha256,
  times,
  memory,
};
writeFileSync(output, JSON.stringify(report, null, 2) + "\n");
console.log(
  JSON.stringify({
    compilerSha256: report.compilerSha256,
    answerSha256,
    medianMs: [...times].sort((a, b) => a - b)[100],
    memory,
  }),
);
