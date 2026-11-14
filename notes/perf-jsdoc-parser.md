# Front-end allocations, round four: JSDoc lists on the parser's buffers, one lazy JSDoc parser per thread

Measured 2026-10-25 at `a0db53ea` (main after #769) on an 18-core Apple Silicon machine, following AGENTS.md
"Performance and memory changes". The item is the third of notes/perf-reparse-cloner.md's "Next" list; two rounds of
allocation profiles put it at 6.8% of webpack's front-end allocations plus 0.6% of its full check in
`new_node_factory` alone.

## Question

Every JSDoc comment the parser handled grew fresh heap vectors: the `Vec<CommentRange>` of a node's candidate comments,
the per-node `Vec<P<Node>>` of parsed comments (then copied into the arena), the `Vec<P<Node>>` of tags and of
comment parts inside `parse_jsdoc_comment_worker`, the text pieces of each `{@link}`, and the nested lists of a
`@typedef`'s or `@callback`'s children, a `@param`'s nested type literal and a `@template`'s type parameters. The
parser already keeps three `*_space` buffers for the comment-text lists of the same code (Go builds all of these in
fresh slices).

For TS files JSDoc is parsed lazily: `parse_jsdoc_for_node` (registered into tsrs_ast's `PARSE_JSDOC_FOR_NODE`) built
a whole `Parser` per node whose JSDoc the checker asked for: `NodeFactory::default()` with its two `Rc<Cell<usize>>`
counters, a clone of the file's `SourceFileParseOptions` (`file_name` string and path), and then the first use of
every scratch buffer allocated again because the parser was new.

## Change

- `Parser` keeps four more scratch buffers next to the existing three: `jsdoc_comment_ranges_space`,
  `jsdoc_tags_space`, `jsdoc_comment_parts_space`, `jsdoc_link_text_space`. They are taken with `mem::take` for the
  duration of one list and put back cleared, like `jsdoc_comments_space`, so a re-entrant use (none is known) finds
  an empty vector and allocates its own rather than aliasing.
- The per-node list of parsed comments and the nested lists inside a tag build on `node_stack`, the parser's one list
  stack ("a nested list pushes above its parent's elements and pops them before returning"): `parse_jsdoc_comments`
  pushes the parsed comments and the caller copies `node_stack[start..]` into the arena (eager) or into the returned
  `Vec` (lazy; tsrs_ast's `resolve_jsdoc` owns that signature and copies it into the file's region, untouched
  here). `parse_nested_type_literal`, `parse_typedef_tag`, `parse_callback_tag_parameters` and
  `parse_template_tag_type_parameters` use `finish_node_list` or `alloc_slice` on their stack range.
- `get_jsdoc_comment_ranges` (public, used by tsrs_ls) is now a wrapper around `append_jsdoc_comment_ranges`, which
  extends and filters the caller's vector.
- `parse_jsdoc_for_node` takes a per-thread parser (`thread_local!` `Cell<Option<Box<Parser>>>`, taken for the call
  and put back after it), initializes only the text state (`Parser::initialize_text_state`: scanner, text, script
  kind, context flags; the parse options are read by `parse_source_file_worker` alone), and recycles the parser
  after the call: `Parser::recycle` rebuilds `new_parser`'s state keeping the factory handle and the scratch
  buffers, so the scanner's number caches, diagnostics, `not_parenthesized_arrow` and every other field start fresh
  and nothing of the previous file (whose text may live in a retired leaf region) is carried over. `Parser` is 832
  bytes; the two moves cost less than one `malloc`.

Where nodes go is unchanged: the factory is the default one (`scratch` off), `P::new` allocates into the thread's
arena or the region `resolve_jsdoc` entered (`owner_region`), and the reparse cloner (#716) is per parser and never
built on the lazy path (TS files do not reparse tags). Nothing in the JSDoc code reads the factory's counters.

## Exactness

- The parser oracle's AST hashes (`crates/tsrs_parser/examples/ast_oracle`, `hash` mode; the dump walks every node's
  `jsdoc()`, so TS files go through the lazy path, and prints the JSDoc diagnostics) are identical between the base
  and candidate builds on all 202,810 `.ts`/`.tsx`/`.js`/`.jsx`/`.mts`/`.cts`/`.mjs`/`.cjs` files of the three
  projects' trees, dependencies included (webpack 55,127, typeorm 30,923, mui-docs 116,760); no panics, no errors.
- Diagnostics (exit code and output bytes) identical in every run of every project below, single-threaded and
  default checkers.
- `cargo test -p tsrs_parser -p tsrs_ast`, `tools/regressions.sh` with the candidate binary, clippy with warnings
  denied, `cargo fmt --check` and the source lint pass.

## Measurement

Stock `--release` builds (fat LTO, stripped), `/usr/bin/time -l` instructions retired (kernel included),
`--noEmit --incremental false --pretty false --singleThreaded`, `RAYON_NUM_THREADS=1`, one warm-up then seven
interleaved base/candidate pairs, medians of the paired changes. Peak RSS with the default checker count, three
pairs. Other agents were building and measuring on the machine throughout; wall time is not quoted.

| project | pairs | instructions base | candidate | change | pairs down | default-checker peak RSS (3 pairs) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| webpack (`-p .`, JS: eager JSDoc, lazy for its `.d.ts`) | 7 | 10.905 G | 10.878 G | **-0.33%** | 6 of 7 | 494 MiB, -0.17% (pairs -0.34% to +1.45%) |
| typeorm (`packages/typeorm`, all lazy) | 7 | 7.700 G | 7.714 G | +0.07% | 2 of 7 | 262 MiB, -0.19% (-1.27% to +0.07%) |
| mui-docs (`docs`, all lazy) | 7 | 37.910 G | 37.805 G | -0.35% | 4 of 7 | 1580 MiB, -0.36% (-1.21% to 0.00%) |

The webpack pairs run from -0.71% to +0.13%. typeorm's row is its second series; the first, under heavier load (one base
run took 24,296 involuntary context switches against about 400 for the others), read +0.62% with 1 of 7 pairs down and
pairs from -4.4% to +5.1%, so it was repeated once the machine was quieter: +0.07%, pairs -0.49% to +0.53%. Both lazy
projects are flat: the lazy path saves about ten heap operations per node with JSDoc and typeorm has only a few
thousand such nodes (26,728 fewer allocations in all), below the spread; mui-docs' -0.35% median at 4 of 7 is noise
around zero for the same reason. The win is webpack's eager path. RSS changes are inside their pairs' spread; the
change retains nothing new (the buffers live in one parser per file and one per checker thread).

Heap allocations per run, exact counts from a counting-allocator build (`alloc` + `realloc` calls, single-threaded;
the uncommitted instrument of notes/perf-reparse-cloner.md):

| project | full check, base | candidate | change | `--noCheck`, base | candidate | change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| webpack | 3,370,378 | 3,217,543 | **-4.5%** | 658,645 | 581,001 | **-11.8%** |
| typeorm | 2,830,075 | 2,803,347 | -0.9% | 4,986,668 | 4,983,654 | -0.1% |
| mui-docs | 11,848,169 | 11,835,751 | -0.1% | 3,254,188 | 3,243,339 | -0.3% |

The counts are deterministic: every run of a build repeats its count exactly. typeorm's `--noCheck` count barely
moves because the lazy path only runs when the checker asks for a node's JSDoc. The remaining JSDoc allocation per
lazily parsed node is the `Vec<P<Node>>` the `ParseJSDocForNodeFn` signature returns (tsrs_ast, #772's territory).

The Linux `bench/count.py` gate on the pull request is the landing number; these are the screening numbers.

## Not done

- Returning an arena slice from `parse_jsdoc_for_node` (one heap vector per lazily parsed node with JSDoc): needs
  the `ParseJSDocForNodeFn` type and `resolve_jsdoc` in tsrs_ast, which #772 is changing.
- `parse_isolated_entity_name` still builds a parser per call (language-service only).
