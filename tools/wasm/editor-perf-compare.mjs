// macOS retained-session release comparison. Both modules keep the standalone CLI.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
const [control, optimized, output] = process.argv.slice(2);
assert.ok(control && optimized && output, "control.wasm optimized.wasm output-directory");
mkdirSync(output, { recursive: true });
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const runnerSha256 = hash(readFileSync(new URL("./editor-perf.mjs", import.meta.url)));
const before = execFileSync("uptime", { encoding: "utf8", timeout: 10000 }).trim();
const results = [];
for (let round = 0; round < 5; round++) {
  for (const side of round % 2 ? ["optimized", "control"] : ["control", "optimized"]) {
    const artifact = side === "control" ? control : optimized;
    const reportPath = join(output, `${round}-${side}.json`);
    const result = spawnSync(
      "/usr/bin/time",
      [
        "-l",
        process.execPath,
        "--expose-gc",
        new URL("./editor-perf.mjs", import.meta.url).pathname,
        artifact,
        reportPath,
      ],
      { encoding: "utf8", timeout: 60000 },
    );
    writeFileSync(join(output, `${round}-${side}.resources.txt`), result.stderr);
    assert.equal(result.status, 0, result.stderr);
    const report = JSON.parse(readFileSync(reportPath, "utf8"));
    const instructions = Number(/(\d+)\s+instructions retired/u.exec(result.stderr)?.[1]);
    const maxRss = Number(/(\d+)\s+maximum resident set size/u.exec(result.stderr)?.[1]);
    const footprint = Number(/(\d+)\s+peak memory footprint/u.exec(result.stderr)?.[1]);
    assert.ok(instructions > 0 && maxRss > 0 && footprint > 0, result.stderr);
    const times = [...report.times].sort((a, b) => a - b);
    results.push({ round, side, instructions, maxRss, footprint, medianMs: times[100], report });
    console.log(JSON.stringify({ round, side, instructions, maxRss, medianMs: times[100] }));
  }
}
for (const entry of results)
  for (const key of ["sourceSha256", "configSha256", "answerSha256", "diagnosticsSha256"])
    assert.equal(entry.report[key], results[0].report[key], key);
const summarize = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  return { median: sorted[2], min: sorted[0], max: sorted[4], values };
};
const summary = Object.fromEntries(
  ["control", "optimized"].map((side) => [
    side,
    Object.fromEntries(
      ["instructions", "maxRss", "footprint", "medianMs"].map((key) => [
        key,
        summarize(results.filter((r) => r.side === side).map((r) => r[key])),
      ]),
    ),
  ]),
);
writeFileSync(
  join(output, "results.json"),
  JSON.stringify(
    {
      kind: "generic retained-session editor stress, not CLI or packaged Chrome",
      runnerSha256,
      before,
      after: execFileSync("uptime", { encoding: "utf8", timeout: 10000 }).trim(),
      errors: 0,
      results,
      summary,
    },
    null,
    2,
  ) + "\n",
);
console.log(JSON.stringify(summary));
