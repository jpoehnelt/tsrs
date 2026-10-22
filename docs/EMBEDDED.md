# Embedded WASM sessions

This fork is pinned to upstream `79ea6d25c011a7f498e02bda7f090308f9fb9ac9`.
The reactor exports `tsrs_input`, `tsrs_session_request`, `tsrs_output`, and `tsrs_output_len`.
`npm/tsrs-wasm/core.js` exposes synchronous and asynchronous transports. Browser hosts must
instantiate asynchronously under normal Chrome WASM policy.

Requests are JSON `{method, params}`. Replies contain `result` or a recoverable `error`.
A trap poisons the transport: replace its worker/reactor. `openSession` opens one session per
reactor; `closeSession` releases it. API snapshots isolate projects. The host releases snapshots
and calls `collectIdle`; WASM has no watcher or timer thread.

## Editor operations

`editor` takes `{snapshot, project, file, operation, request, preferences?}`. `request` uses
LSP UTF-16 positions for hover, completion, completionResolve, definition, references, rename,
renameInfo, signatureHelp, symbols, format, inlayHints, codeActions, and codeFixes.

- `signatureHelp` additionally returns the true `argumentIndex`, `argumentCount`, and UTF-16
  `applicableSpan`, including calls beyond the final rest parameter.
- `codeFixes` preserves provider `fixId` and `fixAllDescription` alongside its workspace edit.
  `supportedCodeFixes` returns registered diagnostic codes without checking the project.
- `combinedCodeFix` takes `{fixId}` and calls that provider only. Unknown IDs produce an error.
  The available compiler providers remain imports, isolated-declaration annotations, and
  incorrectly implemented interfaces; this fork does not add TypeScript 5's parameter-inference provider.
- `refactors` takes `{start, end}` in absolute UTF-16 offsets. It offers expression extraction
  to a global function or an enclosing-block constant, and concatenation-to-template rewriting.
  These are tsrs-only file-local providers. Context-sensitive selections are rejected; this
  is not the complete TypeScript 5 refactor catalogue.
- `docComment` takes an absolute UTF-16 `offset`.

API diagnostic JSON omits only the optional repeated `sourceLines` payload. Absolute offsets,
ECMA line/column positions, nested messages and codes are retained. Position conversion reuses
the source file's existing position map rather than rescanning a minified line per diagnostic.
No new Rust cache or compiler policy is introduced.

## Verification

Run `cargo test --profile dev -p tsrs_api -p tsrs_ls` with the pinned `ts-ref` fixtures,
`cargo clippy --all-targets --config 'build.warnings="deny"'`, `cargo fmt -- --check`, and
`python3 tools/lint/source.py`. `diagnostic_positions_test.rs` independently checks UTF-16
positions across emoji, CRLF, CR, U+2028 and U+2029, including invalid-request recovery.

Build with `tools/wasm/build.sh`; run `node tools/wasm/embedded-test.mjs` against that module.
The consumer integration adds execution-based refactor tests, frozen Apps Script acceptance,
and paired packaged Chrome workloads. Consumer results are recorded in myFunction PR 426;
no compiler CLI speedup or renewed upstream conformance claim follows from these editor tests.
