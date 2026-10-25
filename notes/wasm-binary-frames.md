# Embedded binary frame allocation

`npm/tsrs-wasm/binary.js` previously grew a buffer geometrically, retaining its spare
capacity in the returned view. Each string also allocated temporary UTF-8 bytes.
Large snapshot updates could leave several large backing stores awaiting collection.

The writer now captures large immutable strings, encodes small scalars into bounded
chunks, and writes a final frame with exactly its encoded length. Strings encode
directly into that frame. Raw buffers are captured when visited, preserving ownership
and property evaluation order. Tags, limits, nesting checks and wire bytes are unchanged.
This trades additional bookkeeping for lower transient memory.

## Validation

The focused runtime tests cover Unicode, exact backing-store size, raw ownership,
property evaluation, malformed frames, limits, cycles and nesting. A separate direct
comparison of 2,000 deterministic mixed-value inputs produced identical old/new bytes.
The pinned Go reference remains unchanged; this changes the JS embedding transport,
not compiler behavior.

## Synthetic allocation measurement

Five alternating pairs of fresh Node 24.18 processes on an M3 Pro, using
`/usr/bin/time -l`. Each process validates one round trip and encodes 150 identical
3,768,866-byte frames: five synthetic source files, one synthetic declaration file,
and a 1-MiB raw buffer. Every process produces SHA-256
`085b738e28de48df461664f510de0bfb4c72402d98b31337527ce8be918aca78`.
Counters include process startup, validation and encoding; they are not CLI or
compiler benchmark results. The shared desktop load average was about 2.4.

| Whole-process metric | Previous writer, median (range) | New writer, median (range) |
| --- | --- | --- |
| Peak RSS, bytes | 150,126,592 (142,016,512–158,810,112) | 137,396,224 (137,248,768–137,494,528) |
| Instructions retired | 8,109,446,525 (8,104,549,046–8,110,189,574) | 9,396,934,171 (9,396,187,932–9,400,759,225) |
| Wall time, seconds | 0.29 (0.29–0.29) | 0.31 (0.31–0.31) |

Peak RSS falls 8.5%; retired instructions rise 15.9% and wall time rises about 7%.
The retained frame has no spare backing-store capacity. This is a memory tradeoff,
not a general speedup. Native compiler source is unchanged.
