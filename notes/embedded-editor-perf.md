# Retained editor usage-inference work

On a generic one-file JavaScript editor workload, the optimized reactor retires 28.1% fewer
instructions and uses 34.3% less peak RSS across five alternating release runs. Per-request
latency ranges overlap. This is editor-session evidence, not a compiler CLI speedup.

The workload requests the same parameter-inference fix 201 times after semantic diagnostics.
Every full fix response and diagnostic response has the same SHA-256 on both sides. It uses
the default standard libraries and checker settings. Both modules retain the public standalone
CLI. The two sides use the same production `tools/wasm/build.sh` settings, Node and M3 Pro host.
The shared desktop had a load average of about nine at the start; the alternating order and
complete per-run observations are retained in
[embedded-editor-perf.generated.json](embedded-editor-perf.generated.json).

| Measure | Control median (range) | Optimized median (range) |
| --- | --- | --- |
| Entire-process retired instructions | 17,117,033,593 (17,080,906,844–17,126,984,817) | 12,304,982,207 (12,275,109,673–12,318,990,327) |
| Peak RSS, bytes | 253,034,496 (252,674,048–253,542,400) | 166,363,136 (165,789,696–166,838,272) |
| Peak footprint, bytes | 215,370,344 (214,977,080–215,894,656) | 128,813,696 (128,207,440–129,272,424) |
| Median warm request, ms | 3.627 (3.557–3.717) | 3.438 (3.331–3.736) |

The changes narrow parameter searches to their lexical function and search compiler-validated
text candidates for other symbols. Unicode escapes retain the prior traversal. Editor providers
that do not inspect auto-import exports use the retained snapshot directly. Import-dependent,
unknown and mixed providers retain auto-import preparation. No persistent reference cache,
stack reduction, library exclusion, checker-count change or answer exclusion is introduced.
These editor-only paths do not change ordinary compiler checking. Profiles in a large private
monorepo previously identified reference traversal and scanning during inference; this stress
isolates that operation but does not decompose savings among the changes or compiler/JIT work.

Reproduce on macOS with two release modules built by `tools/wasm/build.sh`:

```sh
node tools/wasm/editor-perf-compare.mjs control.wasm optimized.wasm /tmp/editor-perf
```

The script uses `/usr/bin/time -l` to record instruction and peak-memory counters, asserts every
answer, and records source/configuration/module hashes. Run without concurrent builds or tests.
The control module SHA-256 is
`aac698b42b3d07416c70cc6c24d48de0a0044c11bd6ccd4a10c8e5e647d1c013`;
the optimized module is
`ff37557b3caf0b1f047a230dcef645b2c64746735a5b8772c9ab3dc59ce735d2`.
The baseline is commit `e06b8e80`; the generated report binds both module hashes and probe bytes.
Separate consumer acceptance and packaged-browser validation remain necessary.
