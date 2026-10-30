# Track C: Join

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/c-join.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change is
measured before it is kept; `PERF.md` lists the steps.

## Approach

Rust, standard library only, one file (`src/main.rs`).

1. **Regions.** `regions.csv` becomes a flat array from region id to country index; the country codes are
   sorted first, so the index order is the output order, and `_UNMATCHED` is the last index.
2. **Chunks without a pre-pass.** Section 2.2, which the spec says may be relied on, fixes what the
   generator writes: a note is lowercase words (after an LF inside it comes a letter or a quote), and a name
   never contains an LF. So an LF followed by a digit or by the first byte of a BOM always ends a record, and
   each chunk starts at the first such LF after its nominal start: a short local scan, no pass over the file.
   (An earlier version counted the quotes of every chunk and used the parity; same chunks, one more read of
   202 MB.)
3. **Parse, all threads.** Each record is read byte by byte: an optional BOM, the id, two text fields
   (quoted: scan to the next `"`, a `""` pair counts as one byte; unquoted: up to the comma), the region id,
   the amount in integer cents, and CRLF or LF. `name_bytes` and `note_bytes` are the raw length minus the
   number of `""` pairs.
4. **Distinct names.** Unescaping is a bijection on the raw bytes between the quotes (every `"` inside is
   doubled), so names are compared by their raw bytes. A shared lock-free table gives each distinct name a dense id: a new
   name is copied into a compact arena, then published by CAS as one word (arena offset, length, id); lookups
   compare eight bytes at a time against the arena copy;
   the empty name, quoted or not, is id 0. Each country has a bitset of name ids, set with an atomic OR;
   `names_distinct` is its popcount. The bitsets live in zeroed memory, so only the touched pages cost.
5. **Merge and output.** Per-thread sums, minima and maxima per country are merged in country order; money is
   printed from integer cents (`-0.05`, never `-0.00`).

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md`.
