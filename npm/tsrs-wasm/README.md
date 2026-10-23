# @tsrs/wasm

tsrs (a Rust port of the TypeScript 7 compiler) built as a WebAssembly module for Node 22+ and browsers. It uses the
shared Rust compiler code, compiled for `wasm32-wasip1`, and runs single-threaded.

## Features

- **Type checking** with `--noEmit`, using the same checker as native tsrs.
- **Emit**: JavaScript, declarations (`.d.ts`), source maps and declaration maps.
- **Incremental builds** through `.tsbuildinfo` files, and **project references** with `--build` (sequentially).
- **Text or JSON diagnostics**, with UTF-16 positions in JSON mode.
- **Real or in-memory files** in Node; **in-memory files** in browsers, so checking and emit can run entirely locally.
- **Bundled TypeScript standard libraries**, with no platform-specific binary to install.

The package version follows tsrs and identifies the TypeScript version it ports:
`<tsrs-version>-ts<TypeScript-version>`. For the compiler's supported options and known gaps, see
[compatibility](https://github.com/oxc-project/tsrs/blob/main/docs/COMPATIBILITY.md) and the WASM limits below.

## Command line (Node 22+)

```sh
pnpm dlx @tsrs/wasm -p .
pnpm dlx @tsrs/wasm --version
```

Or install it with `pnpm add -D @tsrs/wasm`, then run `pnpm exec tsrs-wasm -p .`.

The command reads and writes the real file system, prints the compiler's output and exits with its status. A panic
or trap (such as a stack overflow or running out of memory) exits with 5 and a message on stderr; ordinary compiler
statuses are 0-4. Unsupported modes can also exit with 5.

## API

```js
import { tsc } from "@tsrs/wasm";

// The real file system (Node)
const { exitCode, stdout } = await tsc(["-p", "."], { cwd: "/path/to/project" });

// In-memory files, diagnostics as JSON
const result = await tsc(["/src/a.ts", "--noEmit"], {
    cwd: "/src",
    files: { "/src/a.ts": "const s: string = 1;" },
    diagnostics: "json",
});
result.diagnostics; // [{ fileName: "/src/a.ts", pos: 6, end: 7, code: 2322, category: 1, text: "...", ... }]
result.exitCode;    // 2
```

The result contains `exitCode`, `stdout`, `stderr`, optional `diagnostics` and `files`, and `memoryBytes`. With
in-memory files, omit `--noEmit` to emit; `result.files` maps paths to the text of files the compiler wrote.
`memoryBytes` is the module's peak linear memory, including its 80 MiB shadow stack; it does not measure total
process memory.

Options:

- `cwd`: defaults to the process's current directory on Node's real file system, or `/` with in-memory files.
- `files`: absolute paths to contents (`string` or `Uint8Array`), replacing access to the real file system.
- `diagnostics`: `"text"` (default) or `"json"` (the TypeScript API's diagnostic shape, with UTF-16 positions).
- `env`: environment variables visible to the compiler; none are passed by default.
- `tty`, `caseInsensitive`: control terminal formatting and file-system case sensitivity.
- Node: `stdout` / `stderr` can be `"inherit"` to write to the process's fds; output is collected otherwise.
  `stackSizeMb` sets the worker's stack size (default 256 MB).
- Browser: `module` supplies an already compiled `WebAssembly.Module`; `url` overrides where to fetch the module.

`@tsrs/wasm/core` exposes the lower layer: `runTsc(module, request, host, io)` and `runTscAsync(...)` over a custom
`HostFileSystem`, plus `memoryFileSystem(files)`.

## Browsers

The package selects `browser.js` outside Node, exposing the same `tsc(args, options)` API over in-memory files.
Supply project files, configuration and dependency declarations through `files`; standard libraries are bundled.
Run `tsc` in a Web Worker that you create. The browser host does not create a worker automatically, and compilation
runs synchronously after the module loads and instantiates.

By default, the host fetches `tsrs.wasm` next to `browser.js`. Use `url` to point to the deployed asset, or pass
`module` to reuse an already compiled module. Serving the asset as `application/wasm` enables streaming compilation.

From a repository checkout, run `node npm/tsrs-wasm/examples/browser/serve.mjs` to try the
[browser example](https://github.com/oxc-project/tsrs/tree/main/npm/tsrs-wasm/examples/browser).

## Runtime behavior

- **Node**: every call runs in a fresh worker thread with a 256 MB stack (`resourceLimits.stackSizeMb`): the checker
  recurses deeply, and every wasm frame also uses the engine's native stack, so the main thread's stack is too
  small. Compilation within the worker is single-threaded.
- **Bun** ignores the worker stack limit, so there the call runs on the calling thread.

Both hosts cache the compiled module, but create a fresh instance for every call: the compiler keeps process-wide
state, and linear memory never shrinks. Compiler state is not reused between calls. Incremental builds reuse
`.tsbuildinfo` and output files on disk; in memory mode, pass written files back through `files` on the next call.

## Limits

- `--watch`, `--lsp` and the `--api` server are not supported.
- JSON diagnostics with `--build` and `--checkerCostCache` are refused.
- Multiple checkers or builders are not supported. Their flags are accepted, but everything runs on one thread.
- Messages are English only; `--locale` is ignored, as in native tsrs.
- One instance has at most 4 GiB of linear memory (wasm32); the runtime may impose a lower limit.
- Deep recursion can overflow the stack. Safari's worker stack is smaller than Chrome's and Firefox's, so the
  deepest expressions overflow there first.

## Build from source

Install the `wasm32-wasip1` Rust target and Binaryen's `wasm-opt`, then run `tools/wasm/build.sh` from the repository
root. It writes `npm/tsrs-wasm/tsrs.wasm`; run it with `node npm/tsrs-wasm/bin/tsrs-wasm.js -p .`.
See the [build notes](https://github.com/oxc-project/tsrs/blob/main/notes/wasm-build.md) for tool versions and build options.

### Embedded binary requests

`createBinarySession` (async instantiation) and `createBinarySessionSync` retain one reactor and use
`tsrs_session_request_binary`. The JSON session/CLI entry points remain available for existing consumers.
Native embedding consumers can call `Session::handle_native_request` with a borrowed value and receive
`Response::Value` or `Response::Binary`, without encoding JSON. Printing has a separate borrowed byte-slice
entry point. The binary session enables raw source-file/checker responses, including buffers within batches.

Frames begin with `TSRB` and version byte `1`. Values use tags null=0, false=1, true=2, f64=3,
UTF-8 string=4, array=5, map=6, i64=8, raw bytes=9. Numbers and u32 lengths are little-endian;
map keys are length-prefixed UTF-8 strings. Arrays/maps carry element counts; strings/buffers carry byte
counts. Inputs are limited to 64 MiB and 128 nesting levels, with strict UTF-8, finite numbers, duplicate-key
and trailing-byte checks. Request errors return an error envelope and leave the session usable. A WASM
trap requires replacing the reactor. Binary output slices are copied once so callers can retain them
across later requests or memory growth. This is an owned-buffer interface, not zero-copy memory sharing.

The embedded SDK adapter must consume raw `Uint8Array` results and supply raw printing buffers; the
standalone SDK's JSON/base64 adapter is a different transport. No JSON fallback occurs in binary dispatch.

The fork's native editor providers include parameter/this usage inference, missing required or
optional parameters, interface member declarations, implicit-this arrow conversion and their
provider-specific combined fixes. `quickInfo` uses TypeScript 5 display budgeting; regular `hover`
retains expandable native presentation. This does not implement the complete TypeScript fix catalogue.
