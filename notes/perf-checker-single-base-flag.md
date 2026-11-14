# getSingleBaseForNonAugmentingSubtype: the "exists" flag answers the memo without the cache probe

Measured 2026-10-25 at `a0db53ea` (main after #769) on an 18-core Apple Silicon machine shared with other agents'
builds and measurements (load average 60-150), following the owner's performance guide (profile first, attribute,
smallest change, before/after numbers). Unlike the two rounds before it (#715, #774 on Compiler-Unions), this round
profiled the large application projects of bench/projects.json.

## Profile

`samply record --rate 4000` of single-threaded checks (`--singleThreaded`, `RAYON_NUM_THREADS=1`, `--noEmit
--incremental false --pretty false`) of a `--release` build with debug info (`CARGO_PROFILE_RELEASE_DEBUG=true
CARGO_PROFILE_RELEASE_STRIP=false`), self time per symbol weighted by each sample's thread CPU delta (the profile
samples wall time; idle threads weigh nothing). Inlined frames are charged to the function they were inlined
into. Thread CPU: mui-docs 3.9 s, cal-diy 3.7 s, formbricks-web 4.4 s, t3code-server 4.3 s, typeorm 0.6 s,
Compiler-Unions 0.19 s, Compiler 0.12 s.

The single-threaded profile of an application project is led by the kernel's file reads and stats (the front end
on one thread): 39% of mui-docs' CPU, 32% of cal-diy's, 29% of formbricks-web's, 25% of typeorm's, 10% of
t3code-server's. The checker's part is flat: no checker function is above 4.2% of self time on any project, and
the list is the same on every project. Self time, percent of the process's thread CPU:

| function | mui-docs | cal-diy | formbricks-web | t3code-server | typeorm | Compiler-Unions | Compiler |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `SymbolMap::search` | 2.77 | 2.19 | 2.13 | 2.53 | 1.09 | 3.76 | 2.25 |
| `instantiate_type_with_alias_worker` | 1.06 | 2.48 | 2.36 | 4.20 | 0.58 | 0.14 | 0.77 |
| `get_type_at_flow_node` | 0.14 | 0.74 | 0.54 | 1.71 | 1.32 | 2.67 | 3.28 |
| `NameResolver::resolve` | 0.26 | 0.42 | 0.49 | 0.77 | 1.17 | 2.03 | 3.86 |
| `SymbolMap::position` | 1.24 | 0.91 | 0.81 | 1.11 | 0.43 | 1.49 | 1.05 |
| `get_type_of_symbol` | 0.43 | 0.64 | 0.64 | 1.37 | 0.71 | 1.76 | 1.49 |
| `get_property_of_type_worker` | 2.07 | 0.94 | 0.90 | 1.74 | 0.36 | 0.21 | 0.73 |
| `Relation::lookup` | 0.56 | 0.94 | 0.76 | 1.35 | 0.47 | 1.48 | 0.84 |
| `create_union_or_intersection_property` | 2.32 | 0.85 | 0.34 | 0.36 | 0.51 | 1.44 | 0.48 |
| `is_type_related_to` | 0.31 | 0.29 | 0.39 | 0.62 | 0.58 | 2.43 | 1.25 |
| `SymbolMap::insert` (+ its index rehash) | 1.46 (+1.20) | 1.02 (+0.89) | 0.89 (+0.45) | 0.61 (+0.08) | 0.42 | 0.18 | 0.59 |
| `Relater::properties_related_to` | 1.43 | 1.06 | 0.93 | 0.47 | - | 0.68 | 0.29 |
| `get_unmatched_properties_worker_ex` | 0.57 | 0.63 | 0.58 | 0.76 | 0.46 | 1.29 | 0.43 |
| `check_expression_worker` | 0.26 | 0.35 | 0.46 | 0.83 | 0.47 | 0.64 | 1.54 |
| `is_deprecated_symbol` | 0.16 | 0.28 | 0.38 | 0.73 | 0.71 | 0.75 | 1.16 |
| `get_union_type_worker_inner` | 0.72 | 0.72 | 0.46 | 0.59 | 0.25 | 1.37 | - |
| `infer_from_types` | 0.15 | 0.38 | 0.52 | 1.17 | 0.70 | 0.66 | 0.34 |
| `Relater::is_related_to_ex` | 0.38 | 0.52 | 0.60 | 0.63 | - | 1.36 | 0.38 |
| `TypeMapper::map` | 0.33 | 0.88 | 1.05 | 1.36 | - | 0.14 | - |
| `ValueSymbolLinks` store `get` | 0.71 | 0.40 | 0.44 | 0.71 | 0.48 | 0.31 | 0.48 |
| `get_named_members` | 0.54 | 0.63 | 0.48 | 0.41 | 0.25 | 0.13 | 0.73 |
| `get_control_flow_container` | 0.12 | 0.20 | 0.43 | 0.45 | 0.36 | 0.37 | 0.82 |
| `get_normalized_type_worker` | 0.25 | 0.29 | 0.20 | 0.47 | - | 1.09 | 0.42 |
| `get_ready_lazy_member_table_worker` | 0.62 | 0.47 | 0.34 | 1.15 | - | - | - |
| `create_type_reference_ex` | 0.16 | 0.33 | 0.38 | 1.07 | 0.31 | - | 0.28 |
| `get_single_base_for_non_augmenting_subtype` | 0.14 | 0.15 | 0.28 | 0.56 | 0.08 | 0.45 | - |

Where the time inside the top entries goes (per-address samples symbolized with `atos -i`): `SymbolMap::search`,
`position`, `get_property_of_type_worker`, `get_type_of_symbol`, `Relation::lookup`, `ValueSymbolLinks::get`,
`create_type_reference_ex` and `instantiate_type_with_alias_worker` spend their samples on the first load of a type
header, symbol header, member table entry or hash bucket (hashbrown's probe sequence is most of the last three): memory
latency on cold data, as notes/cpu-checker.md found. `create_union_or_intersection_property` is the loop over the
constituents (158 distinct addresses, none above 0.1%). The two functions below were the ones whose samples sat on a
lookup an existing memo already answers.

## Finding

`getSingleBaseForNonAugmentingSubtype` is asked by `getNormalizedType` of every type reference the relater normalizes
(both sides of nearly every relation, through `isRelatedTo`). Its memo is `ObjectFlags::IdenticalBaseTypeCalculated`
on the reference plus an entry in `cached_types` (`CachedTypeKind::EquivalentBaseType`, keyed by type id), and the
port, like tsgo, answered a calculated reference by probing the map:

```rust
if t.object_flags_lazy().intersects(ObjectFlags::IdenticalBaseTypeCalculated) {
    return self.cached_types.get(&key).copied();
}
```

The probe is the function's whole self time: every hot address is hashbrown's probe sequence (`raw.rs`, `map.rs`,
`bitmask.rs`) and the Fx hash of the 8-byte key, in a map shared by all twenty cached type kinds, so the bucket is
rarely in cache. Nearly every reference has no single base (the answer is `None`), so the probe finds nothing.

`ObjectFlags::IdenticalBaseTypeExists` ("has a defined cachedEquivalentBaseType member") is in the ported flag set
with TypeScript's comment: TypeScript's `getSingleBaseForNonAugmentingSubtype` reads it before touching the
reference's `cachedEquivalentBaseType` field. The port (and the Go it follows, which keeps the answer in a map) never
set or read it.

## Change

`get_single_base_for_non_augmenting_subtype` (checker_14.rs) sets `IdenticalBaseTypeExists` on the reference when it
stores the cache entry, and a calculated reference without the bit returns `None` without probing the map; with the
bit it probes as before. `types.rs` only gains a comment on the flag.

Why that is exact: `cached_types` gains an `EquivalentBaseType` entry at exactly one site, the store in this function,
which now sets the bit on the same type in the same step; no site removes such an entry or clears bit 28 on a
reference (`new_type` drops only the `CouldContainTypeVariables`, `MembersResolved` and `WeakTypeMemo` bits; the
`cached_types.remove` in `get_awaited_type_no_alias_ex` is the awaited kind). So a calculated reference without the
bit has no entry, and the probe it skips would have returned `None`. The bit is one of the lazily computed families
the shared-graph overlay already keeps per type (it sits next to `IdenticalBaseTypeCalculated`), and a reference
created with another reference's flags could at most carry a spurious bit, which keeps the probe. A debug-build
assertion checks the skipped probe would have found nothing; `cargo test -p tsrs_checker`, the regression cases and
the seven projects below ran a debug binary with it. The computation itself is unchanged, so resolution order is
unchanged.

## Attribution

A temporary counter in the function (`site-counts`, not committed), one single-threaded debug run per project:

| project | calls | not a class or interface reference | computed (first ask) | memo, no base: probe skipped | memo, base: probe as before |
| --- | ---: | ---: | ---: | ---: | ---: |
| t3code-server | 2,065,720 | 50,897 | 156,611 | 1,485,522 (72%) | 372,690 |
| formbricks-web | 607,878 | 76,240 | 45,092 | 453,278 (75%) | 33,268 |
| mui-docs | 430,980 | 5,823 | 9,475 | 395,268 (92%) | 20,414 |
| cal-diy | 395,909 | 22,844 | 39,997 | 310,666 (78%) | 22,402 |
| Compiler-Unions | 164,753 | 334 | 838 | 163,581 (99.3%) | 0 |
| typeorm | 66,874 | 701 | 8,786 | 57,377 (86%) | 10 |
| Compiler | 7,767 | 310 | 777 | 6,680 (86%) | 0 |

A skipped probe is the Fx hash of the 8-byte key and hashbrown's probe sequence, some 30-45 instructions
(t3code-server's 1.49 M skips are 40-66 M of its 40 G in the pairs below); the rest of the function's sampled time
was the bucket's cache miss, which instruction counts do not see.

## Measurement

Clean fat-LTO `--release` builds with the stock profile, base `a0db53ea` against the change, `/usr/bin/time -l`
instructions retired (kernel included; the macOS screening instrument of notes/perf-icount-round4.md), `--noEmit
--incremental false --pretty false`, interleaved base/candidate pairs with the order alternating each round. The
machine was shared with other agents' builds and measurements throughout (load average 30-150), so wall time is not
reported. Exit code and stdout bytes identical in every run of every cell.

Single-threaded (`--singleThreaded`, `RAYON_NUM_THREADS=1`), fourteen pairs:

| project | instructions base | candidate | change | paired median | pairs lower | peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Compiler-Unions | 2.8952 G | 2.8899 G | **-0.19%** | -0.21% | 14 of 14 | 59.0 -> 59.0 MiB |
| t3code-server | 40.034 G | 39.996 G | -0.09% | -0.13% | 8 of 14 | 713.0 -> 713.0 MiB |
| formbricks-web | 41.038 G | 41.036 G | -0.01% | -0.45% | 9 of 14 | 1,093.1 -> 1,093.1 MiB |
| cal-diy | 32.519 G | 32.552 G | +0.10% | -0.05% | 8 of 14 | 766.4 -> 766.4 MiB |
| mui-docs | 38.245 G | 38.361 G | +0.31% | +0.15% | 6 of 14 | 739.5 -> 739.5 MiB |
| typeorm | 7.906 G | 8.116 G | +2.6% (base alone spread 7.7-8.6 G) | +0.87% | 7 of 14 | 192.0 -> 192.0 MiB |
| Compiler | 1.6988 G | 1.6995 G | +0.04% | +0.08% | 7 of 14 | 55.0 -> 55.0 MiB |

Compiler-Unions, where the two previous rounds measured (#715 -1.6%, #774 -1.0%), is the one project this
instrument resolves: lower in every pair. On the application projects the saving (t3code-server: about 66 M of 40 G)
is inside the kernel-side spread of the front end on this machine, which moves whole-run counts by 0.5-3% between
identical runs; the Linux gate on the pull request, which repeats to 0.001%, is the landing number. The CPU-time
share of the probe (0.56% of t3code-server, 0.45% of Compiler-Unions) is larger than its instruction share because
most of it was the bucket's cache miss.

Default checker count (16 on this machine), three pairs:

| project | instructions base | candidate | peak RSS base | candidate |
| --- | ---: | ---: | ---: | ---: |
| Compiler-Unions | 3.6135 G | 3.6047 G | 82.4 MiB | 83.0 MiB |
| t3code-server | 128.17 G | 128.42 G | 2,028.6 MiB | 2,039.2 MiB |
| formbricks-web | 73.167 G | 72.217 G | 1,940.3 MiB | 1,938.7 MiB |
| cal-diy | 76.975 G | 77.090 G | 1,840.2 MiB | 1,845.2 MiB |
| mui-docs | 94.758 G | 94.374 G | 1,572.4 MiB | 1,574.3 MiB |
| typeorm | 9.4171 G | 8.9414 G | 261.6 MiB | 261.0 MiB |
| Compiler | 2.0470 G | 2.0517 G | 77.5 MiB | 77.1 MiB |

The change adds no field and allocates nothing, so the peak RSS columns (-0.4% to +0.7%) are the run-to-run band of
16 checkers, as in #715's and #774's tables; the instruction counts at this checker count move 1-5% between runs
(stealing).

Gates: `cargo test -p tsrs_checker`, `tools/regressions.sh` with the release binary (50 of 50), `cargo clippy
--all-targets` with warnings denied, `cargo fmt --check`, `node tools/lint/source.mjs`; the debug binary with the
assertion on the seven projects (output identical to the release binary).

## Found on the way, not done (ranked by what it could save)

1. **The symbol-table index rehashes every key's text when it grows** (`SymbolMap::insert` -> hashbrown
   `reserve_rehash` with the closure `index_hash(hash_name(key_at(..)))`, symbol.rs): 1.2% of mui-docs' CPU, 0.9% of
   cal-diy's, 0.45% of formbricks-web's, under 0.1% on t3code-server. Each growth of a table above 16 entries loads
   every entry's symbol and name again and hashes the name (the Fx path for names over 16 bytes: MUI's component
   and CSS property names). By caller: the union and intersection property caches filled by
   `get_union_or_intersection_property` (0.48% mui-docs, 0.24% cal-diy), `add_inherited_members` (0.37%, 0.42%),
   mapped-type member tables and widened object types. Exact fixes: keep the hash's low bits in the index slot (a
   `u32` position has room for 8 bits above a 24-bit position, and the entry's fingerprint already holds the top 12
   bits, so a rehash would read the entries and never a symbol), or reserve `symbols.len() + base_symbols.len()` in
   `add_inherited_members`. Not done: every slot packing makes the probe hash a function of those 20 bits and masks
   the position on each candidate, one to four instructions more in `SymbolMap::search` on every indexed lookup,
   which is 2-4% of CPU on every project; notes/perf-icount-hillclimb.md round 4 saw exactly that trade lose
   (`search` +2.8 G ate the insert savings), and t3code-server has the lookups without the rehashes. The reserve in
   `add_inherited_members` alone is exact and free, worth about 0.4% of mui-docs' and cal-diy's CPU.
2. **`is_deprecated_symbol`**: 0.2-1.2% on every project (Compiler 1.16%, Compiler-Unions 0.75%, t3code-server
   0.73%, typeorm 0.71%), called for every identifier and property access reference; all of it cold loads of the
   declaration nodes and their parents (`get_combined_node_flags`) for a suggestion diagnostic the CLI never prints.
   Skipping it without a suggestion consumer is not exact by construction (`get_parent_of_symbol` can late-bind a
   computed-name parent; the language server reads suggestions). Testing `declarations.len() > 1` before
   `get_parent_of_symbol` is exact (the parent lookup is pure) but worth little.
3. **`get_control_flow_container`**: 0.1-0.8% (Compiler 0.82%, t3code-server 0.45%, formbricks-web 0.43%), two parent
   walks per identifier in `check_identifier` (the declaration's container repeats for every reference of the same
   declaration). A node-keyed direct-mapped cache like `node_file_cache` would be exact (the tree does not change
   during checking) but is a new cache, not an existing memo.
4. **`get_reduced_type_worker` on mui-docs**: 1.44% self, 11.5% inclusive: `some_property_reduces_to_never` for every
   new intersection (name counts over the constituents' property lists, then the union/intersection property of every
   shared name). The self time is the pooled name-count table; the inclusive time is the TypeScript algorithm. For two
   constituents the count could be a membership test in the second's list in the first's order, which is the same
   order; not done.
5. **`create_union_or_intersection_property`**: 2.3% on mui-docs, 0.85% on cal-diy: the constituent loop, 158 distinct
   addresses with none above 0.1%; nothing it computes is answered by a memo.
6. **Property caches look a name up twice**: `get_union_or_intersection_property` misses `cache.lookup(name)` then
   `cache.set(key, prop)` hashes and searches again; `add_inherited_members` does `lookup` then `set` per base
   property. A set under the lookup's `HashedName` would save one hash and one filter test per insert
   (notes/perf-lookup-insert.md warns that the repository-wide form of this regressed).
7. The rest of the list (`SymbolMap::search` and `position`, `get_property_of_type_worker`, `get_type_of_symbol`,
   `Relation::lookup`, the link stores, `create_type_reference_ex`, `get_ready_lazy_member_table_worker`,
   `TypeMapper::map`, `get_type_at_flow_node`) is the probe or the walk itself on cold data; earlier rounds covered
   their shapes (notes/perf-symbol-map-lookups.md, notes/perf-t3code-instantiation.md, notes/perf-icount-hillclimb.md).

Outside the checker: on the application projects the single-threaded run spends 25-39% of its CPU in the kernel's
`read`, `stat` and `open` (the front end on one thread), more than any checker function.
