# Track C: Join

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/c-join.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change is
measured before it is kept; `PERF.md` lists the steps.

## Approach

Rust, standard library only, one file (`src/main.rs`).

1. **Regions.** `regions.csv` is mapped and read twice: once to mark the country codes in a 26^3 table (a
   code is a base-26 number, so the table order is the byte order and the output order; no sort), once to
   fill a flat `u8` array from region id to country index. `_UNMATCHED` is the last index.
2. **Chunks without a pre-pass.** Section 2.2, which the spec says may be relied on, fixes what the
   generator writes: a note is lowercase words (after an LF inside it comes a letter or a quote), and a name
   never contains an LF. So an LF followed by a digit or by the first byte of a BOM always ends a record, and
   each chunk starts at the first such LF after its nominal start: a short local scan, no pass over the file.
   (An earlier version counted the quotes of every chunk and used the parity; same chunks, one more read of
   202 MB.)
3. **Parse, all threads.** Each record is read with SWAR and AVX2: numbers eight digits at a time, the
   closing quote or comma of a field 32 bytes at a time; an optional BOM, the id, two text fields
   (quoted: scan to the next `"`, a `""` pair counts as one byte; unquoted: up to the comma), the region id,
   the amount in integer cents, and CRLF or LF. `name_bytes` and `note_bytes` are the raw length minus the
   number of `""` pairs.
4. **Distinct names.** Unescaping is a bijection on the raw bytes between the quotes (every `"` inside is
   doubled), so names are compared by their raw bytes. A shared lock-free table gives each distinct name a dense id: a new
   name is copied into a compact arena, then published by CAS as one word (arena offset, length, id); lookups
   compare eight bytes at a time against the arena copy;
   the empty name, quoted or not, is id 0. The lookup is a three-stage software pipeline: a record's table
   slot is prefetched, the previous record's name is resolved and its word in the country bitset prefetched,
   and the bit of the record before that is set. Each country has a bitset of name ids, set with an atomic OR;
   `names_distinct` is its popcount. The bitsets live in zeroed memory, so only the touched pages cost.
5. **Merge and output.** Per-thread sums, minima and maxima per country are merged in country order; money is
   printed from integer cents (`-0.05`, never `-0.00`).

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md`.
