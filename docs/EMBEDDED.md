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
LSP UTF-16 positions for hover, quickInfo, completion, completionResolve, definition, references, rename,
renameInfo, signatureHelp, symbols, format, inlayHints, codeActions, and codeFixes.

- `signatureHelp` additionally returns the true `argumentIndex`, `argumentCount`, and UTF-16
  `applicableSpan`, including calls beyond the final rest parameter.
- `codeFixes` preserves provider `fixId` and `fixAllDescription` alongside its workspace edit.
  `supportedCodeFixes` returns registered diagnostic codes without checking the project.
- `combinedCodeFix` takes `{fixId}` and calls that provider only. Unknown IDs produce an error.
  Providers include imports, isolated-declaration annotations, incorrectly implemented interfaces,
  parameter and `this` usage inference, missing required/optional parameters, missing interface
  methods/properties/index signatures, and implicit-`this` arrow conversion. Usage inference and
  missing-parameter providers support their own fix-all IDs and retain cross-file edits. These
  editor providers follow TypeScript 5.9; nested JSDoc tags and call-argument types are retained
  when generating edits. This is not the complete TypeScript code-fix catalogue.
- Usage inference limits parameter references to their lexical function, including captured
  uses in nested closures. Other names use source candidates checked against compiler tokens
  and symbol identity. Escaped identifiers retain AST traversal. Pure inference/member fixes
  do not prepare auto-import exports; import-dependent, unknown and mixed providers still do.
- `codeFixes` includes checker-backed property, name and namespace spelling suggestions with `fixSpelling` fix-all edits. Optional access and UTF-16 positions retain their source semantics; localized diagnostic text is never used as an edit oracle.
- `quickInfo` includes separate `documentation` and `tags` fields for renderers; `contents` retains its LSP Markdown.
- `quickInfo` preserves the TypeScript 5 display budget and called-property presentation; `hover`
  retains the native expandable-hover behavior. CommonJS assignment navigation and constructor
  function instance members are available to embedded editor queries.
- Integer formatting preferences accept both binary integer and JSON numeric representations,
  including explicit zero indentation. Position conversion and edit ownership stay native.
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
A large private monorepo adds execution-based editor tests, sealed acceptance and packaged
browser workloads. Those consumer checks do not establish a compiler CLI speedup or renewed
upstream conformance.


## Rust composition

`tsrs_wasm` is also an rlib. Its `embedded` feature suppresses standalone unmangled ABI symbols,
so an embedding cdylib can provide the existing host ABI once and link other Rust services into
the same allocator and linear memory. `abi::request_with_extension` passes a borrowed live
`Session`, method and parameters to the embedding callback. Returning `None` preserves the normal
compiler dispatch. The callback receives `closeSession`; compiler cleanup runs even if that
callback reports an error. `embedded::dispatch_with_extension` supports native regression tests.
The standalone build explicitly selects cdylib through cargo rustc, retaining fat LTO.

## Typed binary embedding

`createBinarySession`/`createBinarySessionSync` call `tsrs_session_request_binary`. Requests use
bounded typed frames; source-file/checker responses are raw buffers, including in paginated batches.
The SDK must consume owned `Uint8Array` responses and supply raw AST buffers for printing.
`abi::request_binary_with_extension` invokes native callbacks on the same borrowed session.
`Session::handle_native_request` bypasses JSON text and preserves handler parameter validation.
The standalone JSON/CLI adapters remain separate compatibility paths.

`npm/tsrs-wasm/test/binary.test.mjs` covers framing, UTF-8, bounds, nesting, ownership and malformed
inputs. Rust binary session tests cover recovery and independent batch errors. Consumer transport
regressions disable JSON/base64 functions while exercising raw AST reads, printing and batches.

## Declaration and cleanup fixes

The editor API includes const-to-let, unused-identifier deletion/prefixing, missing async modifiers, instance/static member qualification and unreachable-code removal. Each family exposes its own fix-all IDs. The tracker deduplicates repeated edits to a declaration; multiple assigned bindings in one const list produce one replacement. Parameter renames also update their JSDoc names.

An embedding host can pass `readOnlyFiles` as a string array in an `editor` request. These are file identities, not compiler observations. Native language-service policy excludes default libraries, external libraries and these explicit roots from missing-member declaration edits. User-owned declaration interfaces remain eligible. Omitting the array preserves normal program library classification.

Unused and unreachable suggestions must be included with syntactic and semantic diagnostics when a host requests `codeFixes`. The native providers use the same dispatch for those categories. Focused native cleanup/fix-all and explicit-read-only-root regressions cover the added paths.

## Additional TypeScript 5 editor repairs

The native providers also cover local syntax repairs, missing `await`/`new`/`super`, return
corrections, property initialization, abstract members, mapped types, JSDoc migrations and
module/import repairs. Constructor-function and simple Promise-`then` migrations use compiler
bindings and synthesized nodes. Fix-all requests enumerate eligible native syntax when the
TypeScript 7 checker does not emit the corresponding TypeScript 5 suggestion. This is sampled
compatibility work, not exhaustive coverage of every branch of every TypeScript provider.

A planner retains the emit context associated with its synthesized nodes. Alternative actions
share that metadata but have independent edit lists, preserving generated identifiers and
formatting flags. Member fix-all chooses one declaration alternative and deduplicates repeated
uses instead of combining mutually exclusive methods, properties and index signatures. Unused-declaration fix-all removes nested deletion plans when an enclosing
declaration is already being deleted; the tracker's overlap checks remain enabled.

An embedded host can set the `configurationFileName` preference to the original configuration
file in its virtual filesystem. Configuration fixes parse that JSON source natively and edit its
original URI and offsets. Synthetic host configuration files should be included in
`readOnlyFiles`. No compiler observations or expected edits are accepted through this preference.

Focused native regressions cover suggestions without native diagnostics, multiple code-fix
alternatives, generated-name metadata, retained prototype methods and nested unused declarations.
The independent consumer catalogue also compares individual and combined edits without changing
its oracle answers. These checks do not establish complete provider or upstream conformance.
