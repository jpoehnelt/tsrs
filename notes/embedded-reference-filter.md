# Existing identifier tables avoid unrelated reference scans

On the same generic retained-editor release workload, checking the existing source-owned identifier table before text searching retires 63.8% fewer entire-process instructions. Peak RSS rises slightly. This is an editor-session result, not a compiler CLI claim.

| Measure | Previous median (range) | Filtered median (range) |
| --- | ---: | ---: |
| Entire-process retired instructions | 12,307,988,316 (12,279,689,669–12,315,910,270) | 4,449,667,067 (4,434,051,239–4,470,238,983) |
| Peak RSS, bytes | 166,166,528 (165,314,560–166,510,592) | 168,099,840 (166,871,040–168,361,984) |
| Peak footprint, bytes | 128,584,272 (127,699,488–128,944,816) | 130,533,968 (129,370,800–130,796,112) |
| Warm request, ms | 3.3654 (3.3619–3.8470) | 0.0730 (0.0721–0.0735) |

Five alternating runs preserve every full diagnostic and fix reply hash. Both modules retain the standalone CLI and use the same release compiler, standard libraries, production build settings, Node and shared M3 Pro desktop. The generated record contains all source/configuration/module hashes, individual samples and load observations. No builds or tests ran during measurement.

The previous editor-only candidate search repeatedly scanned standard-library text for absent identifiers. SourceFile already owns a lazy identifier set with the same lifetime as its nodes. Rejecting an absent name avoids the scan; candidate token positions, symbol identity, lexical parameter scopes and escaped-identifier traversal remain unchanged. No additional cache or invalidation protocol is introduced. The larger measured peak is retained as a tradeoff. Ordinary compiler checking does not call this editor inference path.

Previous experiments rejected eager general name interning and unrelated checker hash caches; this change reuses an existing source-owned set for a measured editor reference-search hotspot. See [embedded-editor-perf.md](embedded-editor-perf.md) for the earlier batch and its separate baseline.

Reproduce with production release modules:

```sh
node tools/wasm/editor-perf-compare.mjs previous.wasm filtered.wasm /tmp/reference-filter
```

The previous source is `64da3ac5`. The module hashes and complete samples are in [embedded-reference-filter.generated.json](embedded-reference-filter.generated.json). Consumer acceptance and packaged-browser validation are separate gates.
