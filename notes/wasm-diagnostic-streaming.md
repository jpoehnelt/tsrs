# Streaming embedded diagnostic responses

Binary diagnostic replies now encode one owned diagnostic value at a time. The compiler
still computes the complete diagnostic list; no diagnostic is dropped, truncated or
paged. This avoids retaining an additional generic map tree for the complete response.
The snapshot remains alive during callbacks, including when the callback releases its
client handle. Errors stop the visitor. Native value and standalone JSON/CLI replies
keep their existing behavior.

Native tests compare all five per-file methods against their original value replies,
including callback errors, released snapshots and closed sessions. Binary tests compare
exact encoded bytes and batched replies. The pinned Go reference is unchanged.

## Synthetic binary API measurement

Five alternating pairs of fresh Node 24.18 processes on an 11-core M3 Pro. Both WASMs use
the same release settings: opt-level `s`, fat LTO, one codegen unit and `wasm-opt -Os`.
The baseline is `88e46ca2`; the candidate changes diagnostic response construction.
The shared desktop load average was about 5.7, with other applications active.

Run `tools/wasm/diagnostic-memory.mjs <module>` under `/usr/bin/time -l`. Each process
creates a no-lib project containing 40,000 unresolved identifiers and consumes three
complete semantic diagnostic replies. All ten processes return 40,000 code-2304
errors per query and identical complete-answer SHA-256
`c8132f4e17b2c1195c3d19df8345255a69db9a1fc5cb34b004eabadec4084232`.
Source SHA-256 is `5cbe33634977bff4fea2b8ff6ce9a955c73b279978b0dde64f5083db618ff0dc`.
There are no failed processes or query errors.

| Whole-process metric | Before, median (range) | After, median (range) |
| --- | --- | --- |
| Peak RSS, bytes | 352,305,152 (351,911,936–354,664,448) | 319,930,368 (319,684,608–320,618,496) |
| Instructions retired | 18,417,358,894 (18,278,396,295–18,728,760,052) | 11,433,104,225 (11,403,275,093–11,455,027,661) |
| Wall time, seconds | 0.76 (0.72–0.78) | 0.52 (0.52–0.53) |

Peak RSS falls 9.2%, instructions fall 37.9%, and whole-process wall time falls about
32%. These include Node startup, compilation/instantiation, all diagnostic work,
decoding and answer hashing. They are synthetic API transport results, not CLI or
browser-extension benchmarks, and do not prove a hosted runtime memory budget.
The CLI path is unchanged; the compiler project catalog was not rerun for this
transport-only measurement.

Artifact SHA-256:

- Before: `6958f1bd22f70e86fc93bee9d0ecf6b54612b07cf5144726321779180baf4a21`
- After: `0d5937da7ccff55542a5b6d33172b5fd492f45b15a025cd5d4ae595875cc5d47`

## Owned output follow-up

A second pass reserves the final result envelope in the streaming buffer and transfers
that buffer directly for root replies. Nested batches retain their existing framing.
The ABI releases the preceding output at the next request, when all output views
expire, so that allocation can be reused while constructing the next reply.

The same workload and baseline were repeated in five alternating fresh-process pairs;
all answer hashes still match. These measurements are a separate run, not a paired
comparison with the intermediate streaming module above.

| Whole-process metric | Before, median (range) | After, median (range) |
| --- | --- | --- |
| rss | 352845824 (352567296–354598912) | 301694976 (301383680–302825472) |
| instructions | 18121044277 (17819567304–18426799708) | 11393866431 (11359364479–11400874199) |
| seconds | 0.81 (0.79–0.85) | 0.58 (0.57–0.6) |

The counter units are bytes, retired instructions and seconds respectively. Artifact
SHA-256 after this follow-up is `0a3415ddce0a4d00e9ae6e7df3386c319a6a547d6688341390100609f8c3175c`.
Raw counters are in `wasm-diagnostic-owned.generated.json`. This remains a synthetic
API transport measurement; it does not establish hosted memory use.
