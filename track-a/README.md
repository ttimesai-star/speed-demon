# Track A: Ingest

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/a-ingest.md).

The code was written with the help of Claude Code (Anthropic), as Rule 06 allows.

## Approach

Rust, standard library only, one file (`src/main.rs`).

1. `requests.log` is mapped with `mmap` (declared through `extern "C"`, no crate), so the input costs no heap.
   The file is cut into one chunk per hardware thread at line boundaries.
2. **Pass 1, parallel.** Each thread parses its lines by hand: the hour index comes straight from the
   day, month and hour digits (the spec fixes the 72-hour span), the method is skipped, the endpoint is
   hashed while it is scanned and looked up in a shared lock-free open-addressing table (insert by CAS;
   the key is the offset and length of its first occurrence in the mapped file). Each line bumps an atomic
   count, and a 5xx an atomic error count, for its group `hour * 4096 + slot`.
3. A prefix sum over the counts gives every group a contiguous range in one flat `u32` array.
4. **Pass 2, parallel.** The lines are parsed again and each latency is scattered into its group's range
   through an atomic cursor. The order inside a group depends on thread timing, but percentiles depend
   only on the multiset, so the output does not.
5. **Percentiles, parallel by hour.** For each group three nested `select_nth_unstable` calls find the
   nearest-rank p99, then p95 inside the lower part, then p50 — no full sort.
6. Endpoints are sorted once by bytes; each hour's lines are formatted into their own buffer and the 72
   buffers are written in hour order.

Determinism: nothing in the output depends on thread order (5 runs, one hash).

## Dependencies

None (`dependency_count = 0`).

## With another 24 hours

See `PERF.md` for what was tried.
