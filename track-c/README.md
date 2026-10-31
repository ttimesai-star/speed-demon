# Track C: Join

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/c-join.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows. Every change is
measured before it is kept; `PERF.md` lists the steps.

## Approach

Rust, standard library only, one file (`src/main.rs`), statically linked.

1. **Chunks without a pre-pass.** Both files are mapped with `mmap`. Section 2.2, which the spec says may be
   relied on, fixes what the generator writes: a note is lowercase words (after an LF inside it comes a
   letter or a quote), and a name never contains an LF. So an LF followed by a digit or by the first byte of a
   BOM always ends a record, and each chunk starts at the first such LF after its nominal start. There are four
   chunks per thread, taken in turn from an atomic counter, so the threads finish together.
2. **Regions, all threads.** Each thread parses a slice of `regions.csv` twice behind a barrier: first it
   marks the country codes present in a 26^3 table (a code is a base-26 number, so table order is byte order
   and output order; no sort), then it writes `1 + rank` of each region's code into a byte map indexed by
   region id (zeroed memory; 0 means no such region, which is `_UNMATCHED`).
3. **Structure, 64 bytes at a time.** AVX2 compares give bit masks of quotes, commas and LFs; a prefix XOR of
   the quote bits marks the quoted spans (a `""` pair flips twice, so it stays inside), and the separators
   are the commas and LFs outside them: four commas and then the LF per record. The popcount of the quote
   bits inside a field gives its `""` pairs, so `name_bytes` and `note_bytes` are the field length minus one
   minus half its quotes, with no byte-by-byte scan. Numbers are parsed eight digits at a time (SWAR).
4. **Distinct names.** Unescaping is a bijection on the raw bytes between the quotes, so names are hashed on
   their raw bytes. A shared lock-free table maps a name's 64-bit hash to a dense id: an 8-byte slot holds 44
   hash bits and the id, the slot index is 17 more bits, so two names count as equal only when all 61 bits
   match; with 100,000 names that happens with a chance below 10^-8. The table is 1 MB. Ids start at 1, so a
   filled slot is never 0 (the empty name hashes to 0). Each country has a
   bitset of name ids (rows a few cache lines more than 2^20 bits apart, so they do not share L2 sets), set
   with an atomic OR after a plain load; `names_distinct` is its popcount. The lookup is a three-stage
   pipeline: a record's slot is prefetched, the previous record's id is found and its bitset word prefetched
   for writing, the bit of the record before that is set.
5. **Merge and output.** Each thread drops its pages of the input mapping when done (`MADV_DONTNEED`).
   Per-thread sums, minima and maxima per country are merged in country order; money is printed from integer
   cents (`-0.05`, never `-0.00`).

Checked on every step: the published hashes (seed 1 sample and full), the reference run locally (seed 2
sample and full, seeds 3 and 4 sample, seed 1 tiny) and the spec's worked example; five runs give one hash.

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md`.
